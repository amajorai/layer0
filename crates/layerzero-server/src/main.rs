mod routes;
mod state;

use anyhow::Result;
use axum::{http::Method, routing::{delete, get, post}, Router};
use clap::Parser;
use layerzero_core::{config::Config, db::connect, llm::LlmClient};
use std::path::PathBuf;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use routes::{chat, documents, embeddings, graph, models, search};
use state::AppState;

#[derive(Parser)]
#[command(name = "layerzero-server", version, about = "layerzero RAG memory server")]
struct Args {
    #[arg(short, long)]
    config: Option<PathBuf>,
    #[arg(long, env = "LAYERZERO_HOST")]
    host: Option<String>,
    #[arg(short, long, env = "LAYERZERO_PORT")]
    port: Option<u16>,
    #[arg(long, env = "LAYERZERO_LLM_BASE_URL")]
    llm_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "layerzero=info,tower_http=warn".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();
    let mut config = Config::load(args.config.as_ref())?;
    if let Some(h) = args.host { config.server.host = h; }
    if let Some(p) = args.port { config.server.port = p; }
    if let Some(u) = args.llm_url { config.llm.base_url = u; }

    let pool = connect(&config).await?;
    let llm = LlmClient::new(&config.llm)?;

    if !llm.health().await {
        tracing::warn!(
            "LLM server not reachable at {}. Start llama-server or set LAYERZERO_LLM_BASE_URL.",
            config.llm.base_url
        );
    } else {
        info!("LLM server healthy at {}", config.llm.base_url);
    }

    let state = AppState::new(pool, config.clone(), llm);
    let addr = format!("{}:{}", config.server.host, config.server.port);

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PUT, Method::OPTIONS])
        .allow_headers(Any)
        .allow_origin(Any);

    let app = Router::new()
        .route("/health", get(|| async { axum::Json(serde_json::json!({ "status": "ok", "service": "layerzero" })) }))
        .route("/v1/stats", get(documents::get_stats))
        .route("/v1/documents", post(documents::create_document).get(documents::list_documents))
        .route("/v1/documents/:id", get(documents::get_document).delete(documents::delete_document))
        .route("/v1/search", post(search::search))
        .route("/v1/rag", post(search::rag))
        .route("/v1/graph/nodes", post(graph::create_node_route).get(graph::list_nodes_route))
        .route("/v1/graph/nodes/:id", get(graph::get_node_route).delete(graph::delete_node_route))
        .route("/v1/graph/edges", post(graph::create_edge_route).get(graph::list_edges_route))
        .route("/v1/graph/edges/:id", delete(graph::delete_edge_route))
        .route("/v1/graph/query", post(graph::query_graph))
        .route("/v1/embeddings", post(embeddings::create_embeddings))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route("/v1/models", get(models::list_models))
        .route("/v1/models/download", post(models::download_model))
        .route("/v1/models/install-llama", post(models::install_llama))
        .route("/v1/models/:name", delete(models::delete_model))
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    info!("layerzero server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
