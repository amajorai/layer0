//! Interactive ratatui-based config editor.
//!
//! Loads a TOML config file, presents every top-level section's scalar
//! key/value pairs as a navigable list, and lets the user edit values inline
//! while preserving their original types. Saving serializes the edited
//! `toml::Value` back to the same path.

use std::io::{self, Stdout};
use std::path::Path;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};

/// One editable row in the flattened view: which section it belongs to and
/// which key inside that section it points at. The actual value lives in the
/// `toml::Value` document so types are preserved.
struct Row {
    section: String,
    key: String,
}

/// RAII guard that restores the terminal to a sane state on drop, so a panic
/// or early return never leaves the user's terminal in raw / alternate mode.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    fn restore(&mut self) {
        // Best-effort cleanup; ignore errors since we may be unwinding.
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Whole-screen application state.
struct App {
    /// The live config document; edits mutate this in place.
    doc: toml::Value,
    /// Flattened, ordered list of editable section/key rows.
    rows: Vec<Row>,
    list_state: ListState,
    /// `Some(buffer)` while editing the focused row's value inline.
    editing: Option<String>,
    /// Set after a successful edit or load relative to last save.
    modified: bool,
    /// Transient status line message (e.g. "saved", parse errors).
    status: Option<String>,
}

impl App {
    fn new(doc: toml::Value) -> Self {
        let rows = flatten_rows(&doc);
        let mut list_state = ListState::default();
        if !rows.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            doc,
            rows,
            list_state,
            editing: None,
            modified: false,
            status: None,
        }
    }

    fn selected(&self) -> Option<usize> {
        self.list_state.selected()
    }

    fn move_selection(&mut self, delta: i64) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as i64;
        let cur = self.selected().unwrap_or(0) as i64;
        let next = (cur + delta).rem_euclid(len);
        self.list_state.select(Some(next as usize));
    }

    /// Look up the current `toml::Value` for the focused row, if any.
    fn current_value(&self) -> Option<&toml::Value> {
        let idx = self.selected()?;
        let row = self.rows.get(idx)?;
        self.doc
            .get(&row.section)?
            .get(&row.key)
    }

    /// Begin inline editing of the focused scalar. Booleans toggle immediately
    /// instead of opening a text input.
    fn begin_edit(&mut self) {
        let Some(idx) = self.selected() else { return };
        let Some(row) = self.rows.get(idx) else { return };
        let section = row.section.clone();
        let key = row.key.clone();

        let Some(val) = self.doc.get(&section).and_then(|s| s.get(&key)) else {
            return;
        };

        match val {
            toml::Value::Boolean(b) => {
                let toggled = !b;
                self.set_value(&section, &key, toml::Value::Boolean(toggled));
                self.modified = true;
                self.status = Some(format!("{}.{} = {}", section, key, toggled));
            }
            toml::Value::String(_)
            | toml::Value::Integer(_)
            | toml::Value::Float(_) => {
                self.editing = Some(value_to_edit_string(val));
                self.status = None;
            }
            // Non-scalar (array/table/datetime) values are not inline-editable.
            _ => {
                self.status = Some("value type not editable inline".to_string());
            }
        }
    }

    /// Commit the in-progress edit buffer, parsing it back to the original
    /// value's type so we never silently change a number into a string.
    fn commit_edit(&mut self) {
        let Some(buffer) = self.editing.take() else { return };
        let Some(idx) = self.selected() else { return };
        let Some(row) = self.rows.get(idx) else { return };
        let section = row.section.clone();
        let key = row.key.clone();

        let original = self.doc.get(&section).and_then(|s| s.get(&key));
        let new_val = match original {
            Some(toml::Value::Integer(_)) => match buffer.trim().parse::<i64>() {
                Ok(n) => toml::Value::Integer(n),
                Err(_) => {
                    self.status = Some(format!("'{}' is not a valid integer", buffer));
                    return;
                }
            },
            Some(toml::Value::Float(_)) => match buffer.trim().parse::<f64>() {
                Ok(n) => toml::Value::Float(n),
                Err(_) => {
                    self.status = Some(format!("'{}' is not a valid float", buffer));
                    return;
                }
            },
            Some(toml::Value::Boolean(_)) => match buffer.trim().parse::<bool>() {
                Ok(b) => toml::Value::Boolean(b),
                Err(_) => {
                    self.status = Some(format!("'{}' is not a valid bool", buffer));
                    return;
                }
            },
            // Default / strings keep string semantics.
            _ => toml::Value::String(buffer),
        };

        self.set_value(&section, &key, new_val);
        self.modified = true;
        self.status = Some(format!("{}.{} updated", section, key));
    }

    fn cancel_edit(&mut self) {
        self.editing = None;
        self.status = Some("edit cancelled".to_string());
    }

    /// Write a value into the document, creating the section table on demand.
    fn set_value(&mut self, section: &str, key: &str, value: toml::Value) {
        if let Some(toml::Value::Table(tbl)) = self.doc.get_mut(section) {
            tbl.insert(key.to_string(), value);
        } else if let toml::Value::Table(root) = &mut self.doc {
            // Section missing (shouldn't normally happen since rows came from
            // the doc), recreate it defensively.
            let mut tbl = toml::value::Table::new();
            tbl.insert(key.to_string(), value);
            root.insert(section.to_string(), toml::Value::Table(tbl));
        }
    }

    fn save(&mut self, path: &Path) {
        match toml::to_string_pretty(&self.doc) {
            Ok(text) => match std::fs::write(path, text) {
                Ok(()) => {
                    self.modified = false;
                    self.status = Some(format!("saved {}", path.display()));
                }
                Err(e) => {
                    self.status = Some(format!("save failed: {}", e));
                }
            },
            Err(e) => {
                self.status = Some(format!("serialize failed: {}", e));
            }
        }
    }
}

/// Build the ordered, flattened list of editable rows from the document.
/// Only top-level tables (sections) and their scalar keys are surfaced.
fn flatten_rows(doc: &toml::Value) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(table) = doc.as_table() {
        for (section, value) in table {
            if let Some(sec_table) = value.as_table() {
                for key in sec_table.keys() {
                    rows.push(Row {
                        section: section.clone(),
                        key: key.clone(),
                    });
                }
            }
        }
    }
    rows
}

/// Render a `toml::Value` scalar for display in the list / edit buffer.
fn value_display(val: &toml::Value) -> String {
    match val {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(n) => n.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(_) => "[array]".to_string(),
        toml::Value::Table(_) => "{table}".to_string(),
    }
}

/// The string we pre-fill the inline editor with. Same as display for scalars.
fn value_to_edit_string(val: &toml::Value) -> String {
    value_display(val)
}

/// Entry point wired up by the CLI: load, run the event loop, save on demand.
pub fn run(config_path: &Path) -> Result<()> {
    // Load existing config or start from an empty table.
    let doc = if config_path.exists() {
        let text = std::fs::read_to_string(config_path)?;
        text.parse::<toml::Value>()?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let mut app = App::new(doc);

    // The guard restores the terminal on any exit path (including ?/panic).
    let mut guard = TerminalGuard::new()?;
    let result = event_loop(&mut guard.terminal, &mut app, config_path);

    // Explicit restore before returning so the message prints cleanly; Drop
    // would also handle it but doing it here keeps ordering deterministic.
    guard.restore();
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    config_path: &Path,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app, config_path))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        // Only react to key presses (Windows emits press + release events).
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if app.editing.is_some() {
            match key.code {
                KeyCode::Enter => app.commit_edit(),
                KeyCode::Esc => app.cancel_edit(),
                KeyCode::Backspace => {
                    if let Some(buf) = app.editing.as_mut() {
                        buf.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(buf) = app.editing.as_mut() {
                        buf.push(c);
                    }
                }
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Char('q') => break,
            KeyCode::Char('s') => app.save(config_path),
            KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
            KeyCode::Enter => app.begin_edit(),
            _ => {}
        }
    }
    Ok(())
}

fn ui(f: &mut Frame, app: &mut App, config_path: &Path) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Min(3),    // list
            Constraint::Length(3), // edit / status box
            Constraint::Length(2), // footer keybindings
        ])
        .split(f.area());

    render_title(f, chunks[0], config_path);
    render_list(f, chunks[1], app);
    render_edit_or_status(f, chunks[2], app);
    render_footer(f, chunks[3], app);
}

fn render_title(f: &mut Frame, area: Rect, config_path: &Path) {
    let modified_note = format!("layer0 config — {}", config_path.display());
    let title = Paragraph::new(Line::from(Span::styled(
        modified_note,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    f.render_widget(title, area);
}

fn render_list(f: &mut Frame, area: Rect, app: &mut App) {
    let mut items: Vec<ListItem> = Vec::with_capacity(app.rows.len());
    let mut last_section: Option<&str> = None;

    for row in &app.rows {
        // Insert a subtle section header marker when the section changes.
        if last_section != Some(row.section.as_str()) {
            last_section = Some(row.section.as_str());
        }
        let val = app
            .doc
            .get(&row.section)
            .and_then(|s| s.get(&row.key))
            .map(value_display)
            .unwrap_or_default();

        let line = Line::from(vec![
            Span::styled(
                format!("{}.", row.section),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                row.key.clone(),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" = "),
            Span::styled(val, Style::default().fg(Color::Green)),
        ]);
        items.push(ListItem::new(line));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "(empty config — press s to save an empty file)",
            Style::default().fg(Color::DarkGray),
        ))));
    }

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("settings"))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_edit_or_status(f: &mut Frame, area: Rect, app: &App) {
    if let Some(buffer) = &app.editing {
        let label = app
            .selected()
            .and_then(|i| app.rows.get(i))
            .map(|r| format!("{}.{}", r.section, r.key))
            .unwrap_or_else(|| "value".to_string());

        let para = Paragraph::new(Line::from(vec![
            Span::raw(buffer.clone()),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("edit {} (Enter=ok, Esc=cancel)", label)),
        );
        f.render_widget(para, area);
    } else {
        let type_hint = app
            .current_value()
            .map(|v| match v {
                toml::Value::Boolean(_) => "bool (Enter toggles)",
                toml::Value::Integer(_) => "integer",
                toml::Value::Float(_) => "float",
                toml::Value::String(_) => "string",
                _ => "non-scalar",
            })
            .unwrap_or("");

        let msg = app
            .status
            .clone()
            .unwrap_or_else(|| format!("ready  [{}]", type_hint));

        let para = Paragraph::new(Line::from(Span::styled(
            msg,
            Style::default().fg(Color::Magenta),
        )))
        .block(Block::default().borders(Borders::ALL).title("status"));
        f.render_widget(para, area);
    }
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let dirty = if app.modified {
        Span::styled(
            " * modified ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " saved ",
            Style::default().fg(Color::Black).bg(Color::Green),
        )
    };

    let keys = Line::from(vec![
        Span::styled("Up/Down", Style::default().fg(Color::Cyan)),
        Span::raw(" move  "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" edit/toggle  "),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::raw(" cancel  "),
        Span::styled("s", Style::default().fg(Color::Cyan)),
        Span::raw(" save  "),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::raw(" quit  "),
        dirty,
    ]);

    let para = Paragraph::new(keys).block(Block::default().borders(Borders::TOP));
    f.render_widget(para, area);
}
