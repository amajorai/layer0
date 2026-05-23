use axum::{extract::{Path, State}, Json};
use layerzero_core::{
    rag::rag_query,
    retrieval::{retrieve, RagMode},
    types::{RagRequest, RagResponse, SearchRequest, SearchResult},
};

use crate::routes::ApiResult;
use crate::state::AppState;

pub async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> ApiResult<Json<Vec<SearchResult>>> {
    run_search(state, req).await
}

pub async fn search_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Json(mut req): Json<SearchRequest>,
) -> ApiResult<Json<Vec<SearchResult>>> {
    req.database = database;
    req.collection = collection;
    run_search(state, req).await
}

async fn run_search(state: AppState, req: SearchRequest) -> ApiResult<Json<Vec<SearchResult>>> {
    if req.query.trim().is_empty() {
        return Ok(Json(vec![]));
    }

    let model = req.model.as_deref().unwrap_or(state.embedding_model()).to_string();
    let mode = RagMode::parse(req.mode.as_deref().unwrap_or(&state.config.rag.mode));
    let rerank = req.rerank || state.config.rag.rerank;

    let results = retrieve(
        &state.pool, &state.llm, &req.query, &model, req.limit, mode, rerank,
        &req.database, &req.collection,
    )
    .await
    .map_err(anyhow::Error::from)?;

    Ok(Json(results))
}

pub async fn rag(
    State(state): State<AppState>,
    Json(req): Json<RagRequest>,
) -> ApiResult<Json<RagResponse>> {
    run_rag(state, req).await
}

pub async fn rag_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Json(mut req): Json<RagRequest>,
) -> ApiResult<Json<RagResponse>> {
    req.database = database;
    req.collection = collection;
    run_rag(state, req).await
}

async fn run_rag(state: AppState, mut req: RagRequest) -> ApiResult<Json<RagResponse>> {
    let emb_model = req.embedding_model.clone()
        .unwrap_or_else(|| state.embedding_model().to_string());
    let chat_model = req.model.clone()
        .unwrap_or_else(|| state.chat_model().to_string());

    if req.mode.is_none() {
        req.mode = Some(state.config.rag.mode.clone());
    }
    req.rerank = req.rerank || state.config.rag.rerank;

    let resp = rag_query(&state.pool, &state.llm, &req, &emb_model, &chat_model)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(Json(resp))
}
