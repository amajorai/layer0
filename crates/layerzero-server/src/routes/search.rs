use axum::{extract::{Path, State}, Json};
use layerzero_core::{
    embedding::{fts_search, search_similar},
    rag::rag_query,
    rerank::{reciprocal_rank_fusion, rerank},
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
    let threshold = req.threshold.unwrap_or(0.0);
    let db = &req.database;
    let col = &req.collection;

    let vector_results = search_similar(
        &state.pool, &state.llm, &req.query, &model, req.limit * 3, threshold, db, col,
    )
    .await
    .map_err(anyhow::Error::from)?;

    let fts_results = fts_search(&state.pool, &req.query, req.limit * 2, db, col)
        .await
        .unwrap_or_default();

    let mut results = if !fts_results.is_empty() {
        reciprocal_rank_fusion(vec![vector_results, fts_results], 60.0)
    } else {
        vector_results
    };

    if req.use_graph && !results.is_empty() {
        let top = results[0].document.id.clone();
        if let Ok(extras) = graph_expand(&state, &top, db, col).await {
            let existing: std::collections::HashSet<String> =
                results.iter().map(|r| r.document.id.clone()).collect();
            for r in extras {
                if !existing.contains(&r.document.id) {
                    results.push(r);
                }
            }
        }
    }

    if req.rerank && results.len() > 1 {
        results = rerank(&state.llm, &req.query, results, &model).await;
    }

    results.truncate(req.limit);
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

async fn run_rag(state: AppState, req: RagRequest) -> ApiResult<Json<RagResponse>> {
    let emb_model = req.embedding_model.clone()
        .unwrap_or_else(|| state.embedding_model().to_string());
    let chat_model = req.model.clone()
        .unwrap_or_else(|| state.chat_model().to_string());

    let resp = rag_query(&state.pool, &state.llm, &req, &emb_model, &chat_model)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(Json(resp))
}

async fn graph_expand(
    state: &AppState,
    document_id: &str,
    database_name: &str,
    collection_name: &str,
) -> anyhow::Result<Vec<SearchResult>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM graph_nodes WHERE document_id = ? AND database_name = ? AND collection_name = ? LIMIT 1"
    )
    .bind(document_id)
    .bind(database_name)
    .bind(collection_name)
    .fetch_optional(&state.pool)
    .await?;

    if let Some((node_id,)) = row {
        let g = layerzero_core::graph::bfs_traverse(
            &state.pool, &node_id, 1, None, "both", database_name, collection_name,
        )
        .await?;
        Ok(g.documents
            .into_iter()
            .filter(|d| d.id != document_id)
            .map(|d| SearchResult { document: d, score: 0.5, rerank_score: None })
            .collect())
    } else {
        Ok(vec![])
    }
}
