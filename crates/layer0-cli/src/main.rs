mod tui;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use layer0_core::{
    config::Config,
    database::{
        create_collection, create_database, delete_collection, delete_database,
        list_collections, list_databases,
    },
    db::connect,
    embedding::embed_document,
    installer::{download_hf_model, install_llama_cpp, list_installed_models},
    llm::LlmClient,
    rag::rag_query,
    retrieval::{retrieve, RagMode},
    types::RagRequest,
};
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(
    name = "layer0",
    version,
    about = "layer0 - self-hosted RAG memory layer with local LLM support"
)]
struct Cli {
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize layer0 config and directories
    Init,
    /// Start the layer0 HTTP server
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(short, long, default_value = "8080")]
        port: u16,
    },
    /// Store a document in memory
    Store {
        content: Option<String>,
        #[arg(short, long)]
        source: Option<String>,
        #[arg(short, long)]
        metadata: Option<String>,
        #[arg(long, default_value = "true")]
        embed: bool,
        #[arg(long, default_value = "default")]
        database: String,
        #[arg(long, default_value = "default")]
        collection: String,
    },
    /// Search memory
    Search {
        query: String,
        #[arg(short, long, default_value = "5")]
        limit: usize,
        #[arg(long)]
        rerank: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "default")]
        database: String,
        #[arg(long, default_value = "default")]
        collection: String,
    },
    /// Ask a question using RAG
    Ask {
        question: String,
        #[arg(short, long, default_value = "5")]
        limit: usize,
        #[arg(long)]
        no_sources: bool,
        #[arg(long)]
        use_graph: bool,
        #[arg(long, default_value = "default")]
        database: String,
        #[arg(long, default_value = "default")]
        collection: String,
    },
    /// Install llama.cpp
    Install {
        #[command(subcommand)]
        target: InstallTarget,
    },
    /// Model management
    Model {
        #[command(subcommand)]
        action: ModelAction,
    },
    /// Database operations
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Show status
    Status,
    /// Launch MCP server (stdio)
    Mcp,
    /// Update layer0 to the latest GitHub release
    Update,
    /// Edit configuration in an interactive TUI
    Config,
}

#[derive(Subcommand)]
enum InstallTarget {
    Llama,
}

#[derive(Subcommand)]
enum ModelAction {
    List,
    Download {
        repo: String,
        filename: String,
        #[arg(long, default_value = "chat")]
        model_type: String,
        #[arg(long, env = "HF_TOKEN")]
        token: Option<String>,
    },
}

#[derive(Subcommand)]
enum DbAction {
    /// Show document/embedding/node/edge counts
    Stats,
    /// List recent documents
    List {
        #[arg(short, long, default_value = "20")]
        limit: i64,
        #[arg(long, default_value = "default")]
        database: String,
        #[arg(long, default_value = "default")]
        collection: String,
    },
    /// List all databases
    Databases,
    /// List collections in a database
    Collections {
        database: String,
    },
    /// Create a new database
    CreateDatabase {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Create a new collection in a database
    CreateCollection {
        database: String,
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a database and all its data
    DeleteDatabase {
        name: String,
    },
    /// Delete a collection and all its data
    DeleteCollection {
        database: String,
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "layer0=warn".into()))
        .with(tracing_subscriber::fmt::layer().without_time().with_target(false))
        .init();

    let cli = Cli::parse();
    let config = Config::load(cli.config.as_ref())?;

    match cli.command {
        Commands::Init => {
            config.ensure_dirs()?;
            let cfg_path = layer0_core::config::default_data_dir().join("config.toml");
            if cfg_path.exists() {
                println!("config already exists: {}", cfg_path.display());
            } else {
                std::fs::write(&cfg_path, include_str!("../../../config/default.toml"))?;
                println!("created config: {}", cfg_path.display());
            }
            println!("data dir: {}", layer0_core::config::default_data_dir().display());

            // Generate MCP client configs for Claude Code and Cursor in the cwd.
            let mcp_bin = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(if cfg!(windows) { "layer0-mcp.exe" } else { "layer0-mcp" })))
                .filter(|p| p.exists())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "layer0-mcp".to_string());

            let mcp_config = serde_json::json!({
                "mcpServers": {
                    "layer0": { "command": mcp_bin, "args": [] }
                }
            });
            let pretty = serde_json::to_string_pretty(&mcp_config)?;
            for dir in [".claude", ".cursor"] {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    eprintln!("warning: could not create {}: {}", dir, e);
                    continue;
                }
                let path = std::path::Path::new(dir).join("mcp.json");
                if path.exists() {
                    println!("mcp config exists: {}", path.display());
                } else if let Err(e) = std::fs::write(&path, &pretty) {
                    eprintln!("warning: could not write {}: {}", path.display(), e);
                } else {
                    println!("wrote MCP config: {}", path.display());
                }
            }
            if config.chat_uses_local_fallback() {
                println!("chat: local gemma fallback (set ANTHROPIC_API_KEY to use Claude)");
            } else {
                println!("chat: remote backend at {}", config.chat.base_url);
            }
            println!("next: run `layer0 serve` (auto-installs llama.cpp + models on first run)");
        }

        Commands::Serve { host, port } => {
            let bin = std::env::current_exe()?
                .parent()
                .unwrap_or(&PathBuf::from("."))
                .join(if cfg!(windows) { "layer0-server.exe" } else { "layer0-server" });
            if bin.exists() {
                let status = std::process::Command::new(&bin)
                    .arg("--host").arg(&host)
                    .arg("--port").arg(port.to_string())
                    .status()?;
                std::process::exit(status.code().unwrap_or(0));
            }
            eprintln!("layer0-server not found. Build with: cargo build --release -p layer0-server");
            std::process::exit(1);
        }

        Commands::Store { content, source, metadata, embed, database, collection } => {
            let content = match content {
                Some(c) => c,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf.trim().to_string()
                }
            };
            if content.is_empty() {
                return Err(anyhow!("content is empty"));
            }

            let meta: serde_json::Value = metadata
                .as_deref()
                .map(|s| serde_json::from_str(s).unwrap_or_default())
                .unwrap_or_default();

            let pool = connect(&config).await?;
            let id = uuid::Uuid::new_v4().to_string();
            let now = layer0_core::db::now_str();
            let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".into());

            layer0_core::database::ensure_collection(&pool, &database, &collection).await?;

            sqlx::query(
                "INSERT INTO documents (id, content, metadata, source, database_name, collection_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(&content)
            .bind(&meta_str)
            .bind(&source)
            .bind(&database)
            .bind(&collection)
            .bind(&now)
            .bind(&now)
            .execute(&pool)
            .await?;

            if embed {
                let llm = LlmClient::new(&config.llm, &config.effective_chat())?;
                match embed_document(
                    &pool, &llm, &id, &content, &config.llm.embedding_model,
                    &database, &collection,
                    config.chunking.chunk_size, config.chunking.chunk_overlap,
                )
                .await
                {
                    Ok(n) => eprintln!("embedded {} chunk(s)", n),
                    Err(e) => eprintln!("warning: embedding failed: {}", e),
                }

                if config.rag.extract_graph && config.rag.mode != "vector" {
                    let chat_model = config.effective_chat().model;
                    if let Err(e) = layer0_core::graph::extract_and_store_graph(
                        &pool, &llm, &chat_model, &id, &content, &database, &collection,
                    )
                    .await
                    {
                        eprintln!("warning: graph extraction failed: {}", e);
                    }
                }
            }

            println!("{}", id);
        }

        Commands::Search { query, limit, rerank, json, database, collection } => {
            let pool = connect(&config).await?;
            let llm = LlmClient::new(&config.llm, &config.effective_chat())?;
            let mode = RagMode::parse(&config.rag.mode);
            let do_rerank = rerank || config.rag.rerank;
            let results = retrieve(&pool, &llm, &query, &config.llm.embedding_model, limit, mode, do_rerank, &database, &collection).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                if results.is_empty() { println!("no results"); }
                for (i, r) in results.iter().enumerate() {
                    println!("\n[{}] score={:.3}", i + 1, r.score);
                    if let Some(s) = &r.document.source { println!("    source: {}", s); }
                    let preview = r.document.content.chars().take(200).collect::<String>();
                    let ellipsis = if r.document.content.len() > 200 { "..." } else { "" };
                    println!("    {}{}", preview, ellipsis);
                }
            }
        }

        Commands::Ask { question, limit, no_sources, use_graph, database, collection } => {
            let pool = connect(&config).await?;
            let llm = LlmClient::new(&config.llm, &config.effective_chat())?;

            eprintln!("searching knowledge base...");
            let resp = rag_query(
                &pool, &llm,
                &RagRequest {
                    query: question,
                    limit,
                    system_prompt: None,
                    model: None,
                    embedding_model: None,
                    use_graph,
                    rerank: config.rag.rerank,
                    mode: Some(if use_graph { "graph".to_string() } else { config.rag.mode.clone() }),
                    stream: false,
                    database,
                    collection,
                },
                &config.llm.embedding_model,
                &config.effective_chat().model,
            )
            .await?;

            println!("{}", resp.answer);

            if !no_sources && !resp.sources.is_empty() {
                println!("\n--- Sources ---");
                for (i, s) in resp.sources.iter().enumerate() {
                    let preview = s.document.content.chars().take(100).collect::<String>();
                    println!("[{}] {}", i + 1, preview);
                }
            }
        }

        Commands::Install { target: InstallTarget::Llama } => {
            eprintln!("installing llama.cpp...");
            let dir = install_llama_cpp(&config.installer).await?;
            println!("installed: {}", dir.display());
            println!("start: llama-server --model <model.gguf> --port {} --embedding", config.installer.llama_server_port);
        }

        Commands::Model { action } => match action {
            ModelAction::List => {
                let models = list_installed_models(&config.installer.models_dir)?;
                if models.is_empty() {
                    println!("no models in {}", config.installer.models_dir.display());
                    println!("download: layer0 model download <repo> <filename>");
                } else {
                    for m in &models {
                        let size = std::fs::metadata(m).map(|m| m.len()).unwrap_or(0);
                        println!("{} ({:.1} GB)", m.file_name().unwrap_or_default().to_string_lossy(), size as f64 / 1_073_741_824.0);
                    }
                }
            }
            ModelAction::Download { repo, filename, model_type: _, token } => {
                eprintln!("downloading {}...", filename);
                let path = download_hf_model(&config.installer, &repo, &filename, token.as_deref()).await?;
                println!("{}", path.display());
                println!("start: llama-server --model {} --port {} --embedding", path.display(), config.installer.llama_server_port);
            }
        }

        Commands::Db { action } => {
            let pool = connect(&config).await?;
            match action {
                DbAction::Stats => {
                    let docs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents").fetch_one(&pool).await?;
                    let embs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chunks").fetch_one(&pool).await?;
                    let nodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_nodes").fetch_one(&pool).await?;
                    let edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_edges").fetch_one(&pool).await?;
                    println!("documents:  {}", docs);
                    println!("embeddings: {}", embs);
                    println!("nodes:      {}", nodes);
                    println!("edges:      {}", edges);
                    println!("db:         {}", config.database.path.display());
                }
                DbAction::List { limit, database, collection } => {
                    #[derive(sqlx::FromRow)]
                    struct Row { id: String, content: String, source: Option<String> }

                    let rows: Vec<Row> = sqlx::query_as(
                        "SELECT id, content, source FROM documents WHERE database_name = ? AND collection_name = ? ORDER BY created_at DESC LIMIT ?"
                    )
                    .bind(&database).bind(&collection).bind(limit)
                    .fetch_all(&pool)
                    .await?;

                    for r in rows {
                        let preview = r.content.chars().take(80).collect::<String>();
                        println!("{} | {} | {}", r.id, r.source.as_deref().unwrap_or("-"), preview);
                    }
                }
                DbAction::Databases => {
                    let dbs = list_databases(&pool).await?;
                    if dbs.is_empty() {
                        println!("no databases");
                    } else {
                        for db in &dbs {
                            println!("{}", db.name);
                        }
                    }
                }
                DbAction::Collections { database } => {
                    let cols = list_collections(&pool, &database).await?;
                    if cols.is_empty() {
                        println!("no collections in database '{}'", database);
                    } else {
                        for col in &cols {
                            println!("{}/{}", col.database_name, col.name);
                        }
                    }
                }
                DbAction::CreateDatabase { name, description } => {
                    create_database(&pool, &name, description.as_deref()).await?;
                    println!("created database '{}'", name);
                }
                DbAction::CreateCollection { database, name, description } => {
                    create_collection(&pool, &database, &name, description.as_deref()).await?;
                    println!("created collection '{}/{}'", database, name);
                }
                DbAction::DeleteDatabase { name } => {
                    delete_database(&pool, &name).await?;
                    println!("deleted database '{}'", name);
                }
                DbAction::DeleteCollection { database, name } => {
                    delete_collection(&pool, &database, &name).await?;
                    println!("deleted collection '{}/{}'", database, name);
                }
            }
        }

        Commands::Status => {
            let llm = LlmClient::new(&config.llm, &config.effective_chat())?;
            let ok = llm.health().await;
            println!("LLM:      {} ({})", if ok { "online" } else { "offline" }, config.llm.base_url);
            println!("database: {}", config.database.path.display());
            println!("models:   {}", config.installer.models_dir.display());
            if let Ok(pool) = connect(&config).await {
                let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(0);
                println!("docs:     {}", n);
            }
        }

        Commands::Mcp => {
            let bin = std::env::current_exe()?
                .parent()
                .unwrap_or(&PathBuf::from("."))
                .join(if cfg!(windows) { "layer0-mcp.exe" } else { "layer0-mcp" });
            if bin.exists() {
                let s = std::process::Command::new(&bin).status()?;
                std::process::exit(s.code().unwrap_or(0));
            }
            eprintln!("layer0-mcp not found. Build with: cargo build --release -p layer0-mcp");
            std::process::exit(1);
        }

        Commands::Update => {
            eprintln!("checking {} for updates...", config.update.repo);
            match layer0_core::updater::update_now(&config.update).await? {
                layer0_core::updater::UpdateOutcome::UpToDate { version } => {
                    println!("already up to date (v{})", version);
                }
                layer0_core::updater::UpdateOutcome::Updated { from, to } => {
                    println!("updated {} -> {}", from, to);
                    println!("restart any running layer0 processes to apply.");
                }
            }
        }

        Commands::Config => {
            let cfg_path = cli
                .config
                .clone()
                .unwrap_or_else(|| layer0_core::config::default_data_dir().join("config.toml"));
            if !cfg_path.exists() {
                config.ensure_dirs()?;
                std::fs::write(&cfg_path, include_str!("../../../config/default.toml"))?;
                println!("created config: {}", cfg_path.display());
            }
            tui::run(&cfg_path)?;
        }
    }

    Ok(())
}
