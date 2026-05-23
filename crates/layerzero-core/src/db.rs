use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use tracing::info;

use crate::config::Config;

pub async fn connect(config: &Config) -> Result<SqlitePool> {
    config.ensure_dirs()?;
    let url = config.db_url();
    info!("connecting to database: {}", url);

    let pool = SqlitePoolOptions::new()
        .max_connections(config.database.max_connections)
        .connect(&url)
        .await?;

    run_migrations(&pool).await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;
    sqlx::query("PRAGMA foreign_keys=ON").execute(pool).await?;
    sqlx::query("PRAGMA synchronous=NORMAL").execute(pool).await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS documents (
            id          TEXT PRIMARY KEY,
            content     TEXT NOT NULL,
            metadata    TEXT NOT NULL DEFAULT '{}',
            source      TEXT,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS embeddings (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            model       TEXT NOT NULL,
            data        BLOB NOT NULL,
            dimensions  INTEGER NOT NULL,
            created_at  TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_embeddings_doc_model ON embeddings(document_id, model)",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS graph_nodes (
            id          TEXT PRIMARY KEY,
            label       TEXT NOT NULL,
            properties  TEXT NOT NULL DEFAULT '{}',
            document_id TEXT REFERENCES documents(id) ON DELETE SET NULL,
            created_at  TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_nodes_label ON graph_nodes(label)")
        .execute(pool)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_nodes_doc ON graph_nodes(document_id)")
        .execute(pool)
        .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS graph_edges (
            id          TEXT PRIMARY KEY,
            source_id   TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
            target_id   TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
            relation    TEXT NOT NULL,
            weight      REAL NOT NULL DEFAULT 1.0,
            properties  TEXT NOT NULL DEFAULT '{}',
            created_at  TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_edges_source ON graph_edges(source_id)")
        .execute(pool)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_graph_edges_target ON graph_edges(target_id)")
        .execute(pool)
        .await?;

    sqlx::query(
        r#"CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
            content,
            content='documents',
            content_rowid='rowid',
            tokenize='porter ascii'
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TRIGGER IF NOT EXISTS documents_fts_insert
        AFTER INSERT ON documents BEGIN
            INSERT INTO documents_fts(rowid, content) VALUES (new.rowid, new.content);
        END"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TRIGGER IF NOT EXISTS documents_fts_delete
        AFTER DELETE ON documents BEGIN
            INSERT INTO documents_fts(documents_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
        END"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TRIGGER IF NOT EXISTS documents_fts_update
        AFTER UPDATE ON documents BEGIN
            INSERT INTO documents_fts(documents_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
            INSERT INTO documents_fts(rowid, content) VALUES (new.rowid, new.content);
        END"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS models (
            name            TEXT PRIMARY KEY,
            model_type      TEXT NOT NULL,
            path            TEXT,
            context_length  INTEGER,
            dimensions      INTEGER,
            metadata        TEXT NOT NULL DEFAULT '{}',
            created_at      TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    info!("database migrations complete");
    Ok(())
}

pub fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for &f in embedding {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

pub fn deserialize_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
}

pub fn now_str() -> String {
    Utc::now().to_rfc3339()
}

pub fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
