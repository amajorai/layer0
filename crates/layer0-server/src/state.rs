use layer0_core::{config::Config, llm::LlmClient};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Hard cap on the number of simultaneously cached per-database pools.
/// Each open SQLite WAL file holds at least one file descriptor; an unbounded
/// cache would allow a client with write access to exhaust OS fd limits.
const MAX_DB_POOLS: usize = 64;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    pub llm: Arc<LlmClient>,
    chat_model: String,
    db_pools: Arc<RwLock<HashMap<String, SqlitePool>>>,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config, llm: LlmClient) -> Self {
        let chat_model = config.effective_chat().model;
        Self {
            pool,
            config: Arc::new(config),
            llm: Arc::new(llm),
            chat_model,
            db_pools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn embedding_model(&self) -> &str {
        &self.config.llm.embedding_model
    }

    pub fn chat_model(&self) -> &str {
        &self.chat_model
    }

    pub async fn pool_for(&self, database: &str) -> anyhow::Result<SqlitePool> {
        if database == "default" {
            return Ok(self.pool.clone());
        }
        // Validate before any path construction or FS access.
        layer0_core::database::validate_name_pub(database)?;
        {
            let guard = self.db_pools.read().await;
            if let Some(p) = guard.get(database) {
                return Ok(p.clone());
            }
        }
        let p = layer0_core::db::connect_database(&self.config, database).await?;
        {
            let mut guard = self.db_pools.write().await;
            if guard.len() >= MAX_DB_POOLS {
                return Err(anyhow::anyhow!(
                    "too many open databases (limit {}); close unused databases before opening more",
                    MAX_DB_POOLS
                ));
            }
            guard.entry(database.to_string()).or_insert_with(|| p.clone());
        }
        Ok(p)
    }
}
