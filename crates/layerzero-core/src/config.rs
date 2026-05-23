use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub llm: LlmConfig,
    pub chat: ChatConfig,
    pub embeddings: EmbeddingsConfig,
    pub chunking: ChunkingConfig,
    pub rag: RagConfig,
    pub installer: InstallerConfig,
    pub update: UpdateConfig,
}

/// Retrieval strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagConfig {
    /// "hybrid" (vector + graph, then rerank), "vector", or "graph".
    pub mode: String,
    /// Apply a reranking pass to the final results.
    pub rerank: bool,
    /// Extract a knowledge graph (entities + relationships) at ingest time so
    /// graph/hybrid retrieval has data. Skipped automatically in "vector" mode.
    pub extract_graph: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub cors_origins: Vec<String>,
    /// When set, every HTTP request (except /health) must present this key via
    /// `X-API-Key` or `Authorization: Bearer`. Left unset for frictionless local use.
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub path: PathBuf,
    pub max_connections: u32,
}

/// Local embeddings backend (the llama-server sidecar). OpenAI wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub base_url: String,
    pub embedding_model: String,
    pub rerank_model: Option<String>,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
    pub context_length: u32,
}

/// Chat/generation backend. Defaults to Claude via Anthropic's OpenAI-compatible
/// endpoint; any OpenAI-compatible server (Ollama, vLLM, OpenAI, local llama) also works.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    pub dimensions: usize,
    pub batch_size: usize,
    pub search_limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingConfig {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
}

/// GitHub-backed self-update. `repo` is "owner/name".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfig {
    pub repo: String,
    /// Check for a newer release on `serve` startup and log if one exists.
    pub auto_check: bool,
    /// Download and apply a newer release automatically on `serve` startup
    /// (takes effect on the next restart).
    pub auto_update: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallerConfig {
    pub bin_dir: PathBuf,
    pub models_dir: PathBuf,
    pub llama_server_port: u16,
    pub hf_token: Option<String>,
    /// Default embedding model auto-downloaded on first run.
    pub embedding_repo: String,
    pub embedding_file: String,
    /// Default local chat model used as the offline fallback (when no remote
    /// chat key is set and, later, when no ACP client is driving generation).
    pub chat_repo: String,
    pub chat_file: String,
    pub chat_server_port: u16,
    /// Auto-install llama.cpp, auto-download the default models, and auto-start
    /// the sidecar(s) when the server boots.
    pub auto_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = default_data_dir();
        Self {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8080,
                cors_origins: vec!["*".to_string()],
                api_key: None,
            },
            database: DatabaseConfig {
                path: data_dir.join("layerzero.db"),
                max_connections: 5,
            },
            llm: LlmConfig {
                base_url: "http://127.0.0.1:8081".to_string(),
                embedding_model: "nomic-embed-text-v1.5".to_string(),
                rerank_model: None,
                api_key: None,
                timeout_secs: 120,
                context_length: 2048,
            },
            chat: ChatConfig {
                provider: "anthropic".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                model: "claude-haiku-4-5".to_string(),
                api_key: None,
                timeout_secs: 120,
            },
            embeddings: EmbeddingsConfig {
                dimensions: 768,
                batch_size: 16,
                search_limit: 1000,
            },
            chunking: ChunkingConfig {
                chunk_size: 512,
                chunk_overlap: 64,
            },
            rag: RagConfig {
                mode: "hybrid".to_string(),
                rerank: true,
                extract_graph: true,
            },
            installer: InstallerConfig {
                bin_dir: data_dir.join("bin"),
                models_dir: data_dir.join("models"),
                llama_server_port: 8081,
                hf_token: None,
                embedding_repo: "nomic-ai/nomic-embed-text-v1.5-GGUF".to_string(),
                embedding_file: "nomic-embed-text-v1.5.Q4_K_M.gguf".to_string(),
                chat_repo: "bartowski/google_gemma-4-E4B-it-GGUF".to_string(),
                chat_file: "google_gemma-4-E4B-it-Q4_K_M.gguf".to_string(),
                chat_server_port: 8082,
                auto_start: true,
            },
            update: UpdateConfig {
                repo: "amajorai/layerzero".to_string(),
                auto_check: true,
                auto_update: false,
            },
        }
    }
}

pub fn default_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".layerzero")
}

/// Strip a `.gguf`/`.bin` extension to get a model name usable as the OpenAI
/// `model` field for a local llama-server.
pub fn model_stem(filename: &str) -> String {
    std::path::Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string())
}

/// Whether a base URL points at the local machine (so we should manage a sidecar).
pub fn is_local_url(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.contains("127.0.0.1") || u.contains("localhost") || u.contains("[::1]") || u.contains("0.0.0.0")
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
            .set_default("llm.embedding_model", default.llm.embedding_model.clone())?
            .set_default("llm.timeout_secs", default.llm.timeout_secs as i64)?
            .set_default("llm.context_length", default.llm.context_length as i64)?
            .set_default("chat.provider", default.chat.provider.clone())?
            .set_default("chat.base_url", default.chat.base_url.clone())?
            .set_default("chat.model", default.chat.model.clone())?
            .set_default("chat.timeout_secs", default.chat.timeout_secs as i64)?
            .set_default("embeddings.dimensions", default.embeddings.dimensions as i64)?
            .set_default("embeddings.batch_size", default.embeddings.batch_size as i64)?
            .set_default("embeddings.search_limit", default.embeddings.search_limit as i64)?
            .set_default("chunking.chunk_size", default.chunking.chunk_size as i64)?
            .set_default("chunking.chunk_overlap", default.chunking.chunk_overlap as i64)?
            .set_default("rag.mode", default.rag.mode.clone())?
            .set_default("rag.rerank", default.rag.rerank)?
            .set_default("rag.extract_graph", default.rag.extract_graph)?
            .set_default("installer.bin_dir", default.installer.bin_dir.to_string_lossy().as_ref())?
            .set_default("installer.models_dir", default.installer.models_dir.to_string_lossy().as_ref())?
            .set_default("installer.llama_server_port", default.installer.llama_server_port as i64)?
            .set_default("installer.embedding_repo", default.installer.embedding_repo.clone())?
            .set_default("installer.embedding_file", default.installer.embedding_file.clone())?
            .set_default("installer.chat_repo", default.installer.chat_repo.clone())?
            .set_default("installer.chat_file", default.installer.chat_file.clone())?
            .set_default("installer.chat_server_port", default.installer.chat_server_port as i64)?
            .set_default("installer.auto_start", default.installer.auto_start)?
            .set_default("update.repo", default.update.repo.clone())?
            .set_default("update.auto_check", default.update.auto_check)?
            .set_default("update.auto_update", default.update.auto_update)?;

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
                .separator("__")
                .try_parsing(true),
        );

        let cfg = builder.build()?;
        let mut config: Config = cfg.try_deserialize()?;

        // Frictionless auth: pick up the Anthropic key from the standard env var
        // so users never paste secrets into config files.
        if config.chat.api_key.is_none() {
            if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                if !key.trim().is_empty() {
                    config.chat.api_key = Some(key);
                }
            }
        }

        Ok(config)
    }

    pub fn db_url(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.database.path.display())
    }

    /// True when chat falls back to the local llama sidecar (no remote chat key
    /// configured). When false, the configured remote `[chat]` backend is used.
    pub fn chat_uses_local_fallback(&self) -> bool {
        self.chat.api_key.is_none()
    }

    /// The chat backend actually in effect: the configured remote backend when a
    /// key is present, otherwise the local gemma sidecar.
    pub fn effective_chat(&self) -> ChatConfig {
        if self.chat.api_key.is_some() {
            self.chat.clone()
        } else {
            ChatConfig {
                provider: "llama".to_string(),
                base_url: format!("http://127.0.0.1:{}", self.installer.chat_server_port),
                model: model_stem(&self.installer.chat_file),
                api_key: None,
                timeout_secs: self.chat.timeout_secs,
            }
        }
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
