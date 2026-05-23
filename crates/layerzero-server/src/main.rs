mod auth;
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

use routes::{chat, databases, documents, embeddings, graph, models, search};
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

    // Frictionless bootstrap: install llama.cpp, fetch the default model, start
    // the embeddings sidecar. The guard kills the managed child on shutdown.
    let _llama_guard = match layerzero_core::installer::ensure_ready(&config).await {
        Ok(guard) => guard,
        Err(e) => {
            tracing::warn!("auto-start failed ({}). Local models may be unavailable.", e);
            Vec::new()
        }
    };

    // Self-update on startup, per [update] config.
    if config.update.auto_update {
        match layerzero_core::updater::update_now(&config.update).await {
            Ok(layerzero_core::updater::UpdateOutcome::Updated { from, to }) => {
                tracing::warn!("updated layerzero {} -> {} (restart to apply)", from, to)
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("auto-update failed: {}", e),
        }
    } else if config.update.auto_check {
        if let Ok(Some(v)) = layerzero_core::updater::check_latest(&config.update).await {
            tracing::warn!("a newer layerzero release is available: {} (run `layerzero update`)", v);
        }
    }

    let pool = connect(&config).await?;
    let llm = LlmClient::new(&config.llm, &config.effective_chat())?;

    if !llm.health().await {
        tracing::warn!(
            "Embeddings backend not reachable at {}. Start llama-server or set LAYERZERO_LLM_BASE_URL.",
            config.llm.base_url
        );
    } else {
        info!("embeddings backend healthy at {}", config.llm.base_url);
    }

    let state = AppState::new(pool, config.clone(), llm);
    let addr = format!("{}:{}", config.server.host, config.server.port);

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PUT, Method::OPTIONS])
        .allow_headers(Any)
        .allow_origin(Any);

    let app = Router::new()
        // Health
        .route("/health", get(|| async { axum::Json(serde_json::json!({ "status": "ok", "service": "layerzero" })) }))

        // Global stats (all databases)
        .route("/v1/stats", get(documents::get_stats))

        // Global document routes (default database/collection)
        .route("/v1/documents", post(documents::create_document).get(documents::list_documents))
        .route("/v1/documents/:id", get(documents::get_document).delete(documents::delete_document))

        // Global search/rag (default database/collection)
        .route("/v1/search", post(search::search))
        .route("/v1/rag", post(search::rag))

        // Global graph routes (default database/collection)
        .route("/v1/graph/nodes", post(graph::create_node_route).get(graph::list_nodes_route))
        .route("/v1/graph/nodes/:id", get(graph::get_node_route).delete(graph::delete_node_route))
        .route("/v1/graph/edges", post(graph::create_edge_route).get(graph::list_edges_route))
        .route("/v1/graph/edges/:id", delete(graph::delete_edge_route))
        .route("/v1/graph/query", post(graph::query_graph))

        // Database management
        .route("/v1/db", get(databases::list_databases_route).post(databases::create_database_route))
        .route("/v1/db/:database", get(databases::get_database_route).delete(databases::delete_database_route))

        // Collection management
        .route("/v1/db/:database/collections", get(databases::list_collections_route).post(databases::create_collection_route))
        .route("/v1/db/:database/:collection", get(databases::get_collection_route).delete(databases::delete_collection_route))

        // Scoped document routes
        .route("/v1/db/:database/:collection/documents", post(documents::create_document_scoped).get(documents::list_documents_scoped))
        .route("/v1/db/:database/:collection/documents/:id", get(documents::get_document).delete(documents::delete_document))

        // Scoped search & RAG
        .route("/v1/db/:database/:collection/search", post(search::search_scoped))
        .route("/v1/db/:database/:collection/rag", post(search::rag_scoped))

        // Scoped graph routes
        .route("/v1/db/:database/:collection/graph/nodes", post(graph::create_node_scoped).get(graph::list_nodes_scoped))
        .route("/v1/db/:database/:collection/graph/nodes/:id", get(graph::get_node_route).delete(graph::delete_node_route))
        .route("/v1/db/:database/:collection/graph/edges", post(graph::create_edge_route).get(graph::list_edges_scoped))
        .route("/v1/db/:database/:collection/graph/edges/:id", delete(graph::delete_edge_route))
        .route("/v1/db/:database/:collection/graph/query", post(graph::query_graph_scoped))

        // Scoped stats
        .route("/v1/db/:database/:collection/stats", get(documents::get_stats_scoped))

        // OpenAI-compatible endpoints (global)
        .route("/v1/embeddings", post(embeddings::create_embeddings))
        .route("/v1/chat/completions", post(chat::chat_completions))

        // Model management
        .route("/v1/models", get(models::list_models))
        .route("/v1/models/download", post(models::download_model))
        .route("/v1/models/install-llama", post(models::install_llama))
        .route("/v1/models/:name", delete(models::delete_model))

        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(state, auth::require_api_key))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    info!("layerzero server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
