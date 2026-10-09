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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_and_deleted_databases_cannot_reopen_and_concurrent_cache_misses_share_one_pool(
    ) {
        let root = std::env::temp_dir().join(format!("layer0-state-{}", uuid::Uuid::new_v4()));
        let mut config = Config::default();
        config.database.path = root.join("default.db");
        let pool = layer0_core::db::connect(&config).await.unwrap();
        let llm = LlmClient::new(&config.llm, &config.chat).unwrap();
        let state = AppState::new(pool, config, llm);
        assert!(state.pool_for("missing").await.is_err());
        assert!(!state.config.databases_dir().join("missing.db").exists());
        layer0_core::database::create_database(&state.pool, &state.config, "shared", None)
            .await
            .unwrap();
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let state = state.clone();
            tasks.push(tokio::spawn(async move {
                state.pool_for("shared").await.unwrap()
            }));
        }
        let mut opened = Vec::new();
        for task in tasks {
            opened.push(task.await.unwrap());
        }
        assert_eq!(state.db_pools.read().await.len(), 1);
        assert!(state.delete_database("shared").await.unwrap());
        assert!(opened.iter().all(SqlitePool::is_closed));
        assert!(state.pool_for("shared").await.is_err());
        assert!(!state.config.databases_dir().join("shared.db").exists());
        layer0_core::database::create_database(&state.pool, &state.config, "shared", None)
            .await
            .unwrap();
        assert!(!state.pool_for("shared").await.unwrap().is_closed());
        state.delete_database("shared").await.unwrap();
        drop(opened);
        state.pool.close().await;
        drop(state);
        for attempt in 1..=20 {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => break,
                Err(_) if cfg!(windows) && attempt < 20 => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => panic!("failed to remove test database directory: {error}"),
            }
        }
    }
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

    pub async fn delete_database(&self, database: &str) -> anyhow::Result<bool> {
        layer0_core::database::validate_name_pub(database)?;
        anyhow::ensure!(database != "default", "Cannot delete default database");
        let mut pools = self.db_pools.write().await;
        if let Some(pool) = pools.remove(database) {
            pool.close().await;
        }
        layer0_core::database::delete_database(&self.pool, &self.config, database).await
    }

    pub async fn pool_for(&self, database: &str) -> anyhow::Result<SqlitePool> {
        if database == "default" {
            return Ok(self.pool.clone());
        }
        // Validate before any path construction or FS access.
        layer0_core::database::validate_name_pub(database)?;
        // Serialize cache misses so admission precedes opening or migrating a pool.
        let mut guard = self.db_pools.write().await;
        anyhow::ensure!(
            layer0_core::database::get_database(&self.pool, database)
                .await?
                .is_some(),
            "Database does not exist"
        );
        if let Some(pool) = guard.get(database) {
            return Ok(pool.clone());
        }
        if guard.len() >= MAX_DB_POOLS {
            return Err(anyhow::anyhow!(
                "too many open databases (limit {})",
                MAX_DB_POOLS
            ));
        }
        let p = layer0_core::db::connect_database(&self.config, database).await?;
        guard.insert(database.to_owned(), p.clone());
        Ok(p)
    }
}
