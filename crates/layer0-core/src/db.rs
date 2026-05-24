use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use std::sync::Once;
use tracing::info;

use crate::config::Config;

static VEC_INIT: Once = Once::new();

/// Register the sqlite-vec extension so every connection sqlx opens exposes the
/// `vec0` virtual table module. Must run before any connection is created.
fn register_sqlite_vec() {
    VEC_INIT.call_once(|| unsafe {
        libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

pub async fn connect(config: &Config) -> Result<SqlitePool> {
    config.ensure_dirs()?;
    register_sqlite_vec();
    let url = config.db_url();
    info!("connecting to database: {}", url);

    let pool = SqlitePoolOptions::new()
        .max_connections(config.database.max_connections)
        .connect(&url)
        .await?;

    run_migrations(&pool, config.embeddings.dimensions).await?;
    Ok(pool)
}

pub async fn connect_database(config: &Config, database: &str) -> Result<SqlitePool> {
    if database == "default" {
        return connect(config).await;
    }
    config.ensure_dirs()?;
    register_sqlite_vec();
    let url = config.db_url_for(database);
    info!("connecting to database: {}", url);
    let pool = SqlitePoolOptions::new()
        .max_connections(config.database.max_connections)
        .connect(&url)
        .await?;
    run_migrations(&pool, config.embeddings.dimensions).await?;
    Ok(pool)
}

async fn apply_migration(pool: &SqlitePool, name: &str, sql: &str) -> Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?"
    )
    .bind(name)
    .fetch_one(pool)
    .await?;

    if !exists {
        sqlx::query(sql).execute(pool).await?;
        sqlx::query("INSERT INTO _migrations (name, applied_at) VALUES (?, ?)")
            .bind(name)
            .bind(now_str())
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn run_migrations(pool: &SqlitePool, embedding_dims: usize) -> Result<()> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;
    sqlx::query("PRAGMA foreign_keys=ON").execute(pool).await?;
    sqlx::query("PRAGMA synchronous=NORMAL").execute(pool).await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS _migrations (
            name       TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS documents (
            id              TEXT PRIMARY KEY,
            content         TEXT NOT NULL,
            metadata        TEXT NOT NULL DEFAULT '{}',
            source          TEXT,
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL
        )"#,
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

    // Named databases and collections registry
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS databases (
            name        TEXT PRIMARY KEY,
            description TEXT,
            created_at  TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS collections (
            database_name TEXT NOT NULL,
            name          TEXT NOT NULL,
            description   TEXT,
            created_at    TEXT NOT NULL,
            PRIMARY KEY (database_name, name)
        )"#,
    )
    .execute(pool)
    .await?;

    // Ensure the default database and collection exist
    let now = now_str();
    sqlx::query(
        "INSERT OR IGNORE INTO databases (name, created_at) VALUES ('default', ?)"
    )
    .bind(&now)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT OR IGNORE INTO collections (database_name, name, created_at) VALUES ('default', 'default', ?)"
    )
    .bind(&now)
    .execute(pool)
    .await?;

    // Additive column migrations using the _migrations table
    apply_migration(
        pool,
        "add_database_name_to_documents",
        "ALTER TABLE documents ADD COLUMN database_name TEXT NOT NULL DEFAULT 'default'",
    )
    .await?;

    apply_migration(
        pool,
        "add_collection_name_to_documents",
        "ALTER TABLE documents ADD COLUMN collection_name TEXT NOT NULL DEFAULT 'default'",
    )
    .await?;

    apply_migration(
        pool,
        "add_database_name_to_graph_nodes",
        "ALTER TABLE graph_nodes ADD COLUMN database_name TEXT NOT NULL DEFAULT 'default'",
    )
    .await?;

    apply_migration(
        pool,
        "add_collection_name_to_graph_nodes",
        "ALTER TABLE graph_nodes ADD COLUMN collection_name TEXT NOT NULL DEFAULT 'default'",
    )
    .await?;

    apply_migration(
        pool,
        "idx_documents_db_col",
        "CREATE INDEX IF NOT EXISTS idx_documents_db_col ON documents(database_name, collection_name)",
    )
    .await?;

    apply_migration(
        pool,
        "idx_graph_nodes_db_col",
        "CREATE INDEX IF NOT EXISTS idx_graph_nodes_db_col ON graph_nodes(database_name, collection_name)",
    )
    .await?;

    // Chunk-level storage: documents are split into overlapping chunks, each with
    // its own embedding. `chunks.rowid` is the foreign rowid into `vec_chunks`.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS chunks (
            rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
            id              TEXT NOT NULL UNIQUE,
            document_id     TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            chunk_index     INTEGER NOT NULL,
            content         TEXT NOT NULL,
            database_name   TEXT NOT NULL DEFAULT 'default',
            collection_name TEXT NOT NULL DEFAULT 'default',
            created_at      TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_chunks_doc ON chunks(document_id)")
        .execute(pool)
        .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_chunks_db_col ON chunks(database_name, collection_name)",
    )
    .execute(pool)
    .await?;

    // sqlite-vec ANN index. Dimension is fixed at first init; changing the
    // embedding model to a different dimension requires recreating this table.
    sqlx::query(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(embedding float[{}] distance_metric=cosine)",
        embedding_dims
    ))
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
