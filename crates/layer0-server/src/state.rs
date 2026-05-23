use layer0_core::{config::Config, llm::LlmClient};
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    pub llm: Arc<LlmClient>,
    chat_model: String,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config, llm: LlmClient) -> Self {
        let chat_model = config.effective_chat().model;
        Self { pool, config: Arc::new(config), llm: Arc::new(llm), chat_model }
    }

    pub fn embedding_model(&self) -> &str {
        &self.config.llm.embedding_model
    }

    pub fn chat_model(&self) -> &str {
        &self.chat_model
    }
}
