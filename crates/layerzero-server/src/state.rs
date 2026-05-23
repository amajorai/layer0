use layerzero_core::{config::Config, llm::LlmClient};
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    pub llm: Arc<LlmClient>,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config, llm: LlmClient) -> Self {
        Self { pool, config: Arc::new(config), llm: Arc::new(llm) }
    }

    pub fn embedding_model(&self) -> &str {
        &self.config.llm.embedding_model
    }

    pub fn chat_model(&self) -> &str {
        &self.config.llm.chat_model
    }
}
