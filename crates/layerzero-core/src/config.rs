use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub llm: LlmConfig,
    pub embeddings: EmbeddingsConfig,
    pub installer: InstallerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub cors_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub path: PathBuf,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub base_url: String,
    pub chat_model: String,
    pub embedding_model: String,
    pub rerank_model: Option<String>,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
    pub context_length: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    pub dimensions: usize,
    pub batch_size: usize,
    pub search_limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallerConfig {
    pub bin_dir: PathBuf,
    pub models_dir: PathBuf,
    pub llama_server_port: u16,
    pub hf_token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = default_data_dir();
        Self {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8080,
                cors_origins: vec!["*".to_string()],
            },
            database: DatabaseConfig {
                path: data_dir.join("layerzero.db"),
                max_connections: 5,
            },
            llm: LlmConfig {
                base_url: "http://127.0.0.1:8081".to_string(),
                chat_model: "local".to_string(),
                embedding_model: "local".to_string(),
                rerank_model: None,
                api_key: None,
                timeout_secs: 120,
                context_length: 4096,
            },
            embeddings: EmbeddingsConfig {
                dimensions: 1536,
                batch_size: 16,
                search_limit: 1000,
            },
            installer: InstallerConfig {
                bin_dir: data_dir.join("bin"),
                models_dir: data_dir.join("models"),
                llama_server_port: 8081,
                hf_token: None,
            },
        }
    }
}

pub fn default_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".layerzero")
}

impl Config {
    pub fn load(config_path: Option<&PathBuf>) -> Result<Self> {
        let default = Self::default();

        let mut builder = config::Config::builder()
            .set_default("server.host", default.server.host.clone())?
            .set_default("server.port", default.server.port as i64)?
            .set_default("server.cors_origins", vec!["*"])?
            .set_default("database.path", default.database.path.to_string_lossy().as_ref())?
            .set_default("database.max_connections", default.database.max_connections as i64)?
            .set_default("llm.base_url", default.llm.base_url.clone())?
            .set_default("llm.chat_model", default.llm.chat_model.clone())?
            .set_default("llm.embedding_model", default.llm.embedding_model.clone())?
            .set_default("llm.timeout_secs", default.llm.timeout_secs as i64)?
            .set_default("llm.context_length", default.llm.context_length as i64)?
            .set_default("embeddings.dimensions", default.embeddings.dimensions as i64)?
            .set_default("embeddings.batch_size", default.embeddings.batch_size as i64)?
            .set_default("embeddings.search_limit", default.embeddings.search_limit as i64)?
            .set_default("installer.bin_dir", default.installer.bin_dir.to_string_lossy().as_ref())?
            .set_default("installer.models_dir", default.installer.models_dir.to_string_lossy().as_ref())?
            .set_default("installer.llama_server_port", default.installer.llama_server_port as i64)?;

        let global_config = default_data_dir().join("config.toml");
        if global_config.exists() {
            builder = builder.add_source(config::File::from(global_config).required(false));
        }

        if let Some(path) = config_path {
            if path.exists() {
                builder = builder.add_source(config::File::from(path.as_ref()).required(true));
            }
        }

        builder = builder.add_source(
            config::Environment::with_prefix("LAYERZERO")
                .separator("_")
                .try_parsing(true),
        );

        let cfg = builder.build()?;
        Ok(cfg.try_deserialize()?)
    }

    pub fn db_url(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.database.path.display())
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(default_data_dir())?;
        std::fs::create_dir_all(&self.installer.bin_dir)?;
        std::fs::create_dir_all(&self.installer.models_dir)?;
        if let Some(parent) = self.database.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}
