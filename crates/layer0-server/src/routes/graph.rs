use axum::{extract::{Path, Query, State}, Json};
use chrono::Utc;
use layer0_core::{
    graph::{bfs_traverse, create_edge, create_node, delete_edge, delete_node, find_nodes_by_label, get_node, list_edges, list_edges_in_collection, list_nodes},
    types::{GraphEdge, GraphNode, GraphQueryRequest, GraphSearchResult},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::routes::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn create_node_route(
    State(state): State<AppState>,
    Json(req): Json<GraphNode>,
) -> ApiResult<Json<GraphNode>> {
    let node = GraphNode {
        id: if req.id.is_empty() { Uuid::new_v4().to_string() } else { req.id },
        label: req.label,
        properties: req.properties,
        document_id: req.document_id,
        database_name: "default".to_string(),
        collection_name: "default".to_string(),
        created_at: Utc::now(),
    };
    create_node(&state.pool, &node).await.map_err(anyhow::Error::from)?;
    Ok(Json(node))
}

pub async fn create_node_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Json(req): Json<GraphNode>,
) -> ApiResult<Json<GraphNode>> {
    let node = GraphNode {
        id: if req.id.is_empty() { Uuid::new_v4().to_string() } else { req.id },
        label: req.label,
        properties: req.properties,
        document_id: req.document_id,
        database_name: database,
        collection_name: collection,
        created_at: Utc::now(),
    };
    create_node(&state.pool, &node).await.map_err(anyhow::Error::from)?;
    Ok(Json(node))
}

pub async fn get_node_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<GraphNode>> {
    match get_node(&state.pool, &id).await.map_err(anyhow::Error::from)? {
        Some(n) => Ok(Json(n)),
        None => Err(ApiError::NotFound(format!("node {} not found", id))),
    }
}

pub async fn delete_node_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    if delete_node(&state.pool, &id).await.map_err(anyhow::Error::from)? {
        Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
    } else {
        Err(ApiError::NotFound(format!("node {} not found", id)))
    }
}

pub async fn create_edge_route(
    State(state): State<AppState>,
    Json(req): Json<GraphEdge>,
) -> ApiResult<Json<GraphEdge>> {
    let edge = GraphEdge {
        id: if req.id.is_empty() { Uuid::new_v4().to_string() } else { req.id },
        source_id: req.source_id,
        target_id: req.target_id,
        relation: req.relation,
        weight: req.weight,
        properties: req.properties,
        created_at: Utc::now(),
    };
    create_edge(&state.pool, &edge).await.map_err(anyhow::Error::from)?;
    Ok(Json(edge))
}

pub async fn delete_edge_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    if delete_edge(&state.pool, &id).await.map_err(anyhow::Error::from)? {
        Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
    } else {
        Err(ApiError::NotFound(format!("edge {} not found", id)))
    }
}

pub async fn query_graph(
    State(state): State<AppState>,
    Json(req): Json<GraphQueryRequest>,
) -> ApiResult<Json<GraphSearchResult>> {
    run_graph_query(state, req).await
}

pub async fn query_graph_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Json(mut req): Json<GraphQueryRequest>,
) -> ApiResult<Json<GraphSearchResult>> {
    req.database = database;
    req.collection = collection;
    run_graph_query(state, req).await
}

async fn run_graph_query(state: AppState, req: GraphQueryRequest) -> ApiResult<Json<GraphSearchResult>> {
    let db = &req.database;
    let col = &req.collection;

    let start_id = if let Some(id) = &req.start_node_id {
        id.clone()
    } else if let Some(label) = &req.start_label {
        find_nodes_by_label(&state.pool, label, db, col)
            .await
            .map_err(anyhow::Error::from)?
            .into_iter()
            .next()
            .map(|n| n.id)
            .ok_or_else(|| ApiError::NotFound(format!("no node with label '{}'", label)))?
    } else {
        return Err(ApiError::BadRequest("start_node_id or start_label required".into()));
    };

    let dir = req.direction.as_deref().unwrap_or("both");
    let result = bfs_traverse(&state.pool, &start_id, req.depth, req.relation.as_deref(), dir, db, col)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list_nodes_route(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<GraphNode>>> {
    Ok(Json(
        list_nodes(&state.pool, q.limit.unwrap_or(50), q.offset.unwrap_or(0), "default", "default")
            .await
            .map_err(anyhow::Error::from)?,
    ))
}

pub async fn list_nodes_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<GraphNode>>> {
    Ok(Json(
        list_nodes(&state.pool, q.limit.unwrap_or(50), q.offset.unwrap_or(0), &database, &collection)
            .await
            .map_err(anyhow::Error::from)?,
    ))
}

pub async fn list_edges_route(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<GraphEdge>>> {
    Ok(Json(
        list_edges(&state.pool, q.limit.unwrap_or(50), q.offset.unwrap_or(0))
            .await
            .map_err(anyhow::Error::from)?,
    ))
}

pub async fn list_edges_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<GraphEdge>>> {
    Ok(Json(
        list_edges_in_collection(
            &state.pool,
            q.limit.unwrap_or(50),
            q.offset.unwrap_or(0),
            &database,
            &collection,
        )
        .await
        .map_err(anyhow::Error::from)?,
    ))
}
