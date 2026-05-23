use axum::{extract::{Path, Query, State}, Json};
use chrono::Utc;
use layerzero_core::{
    database::ensure_collection,
    db::now_str,
    embedding::{embed_document, fetch_document},
    graph::{create_edge, create_node, find_nodes_by_label},
    types::{CreateDocumentRequest, Document, GraphEdge, GraphNode, Stats},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::routes::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn create_document(
    State(state): State<AppState>,
    Json(req): Json<CreateDocumentRequest>,
) -> ApiResult<Json<Document>> {
    create_document_in(state, "default", "default", req).await
}

pub async fn create_document_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Json(mut req): Json<CreateDocumentRequest>,
) -> ApiResult<Json<Document>> {
    req.database = database.clone();
    req.collection = collection.clone();
    create_document_in(state, &database, &collection, req).await
}

async fn create_document_in(
    state: AppState,
    database: &str,
    collection: &str,
    req: CreateDocumentRequest,
) -> ApiResult<Json<Document>> {
    if req.content.trim().is_empty() {
        return Err(ApiError::BadRequest("content cannot be empty".into()));
    }

    ensure_collection(&state.pool, database, collection)
        .await
        .map_err(anyhow::Error::from)?;

    let id = Uuid::new_v4().to_string();
    let now = now_str();
    let metadata = serde_json::to_string(&req.metadata).unwrap_or_else(|_| "{}".into());

    sqlx::query(
        "INSERT INTO documents (id, content, metadata, source, database_name, collection_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id).bind(&req.content).bind(&metadata).bind(&req.source)
    .bind(database).bind(collection)
    .bind(&now).bind(&now)
    .execute(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;

    if req.embed {
        let model = state.embedding_model().to_string();
        if let Err(e) = embed_document(
            &state.pool, &state.llm, &id, &req.content, &model,
            database, collection,
            state.config.chunking.chunk_size, state.config.chunking.chunk_overlap,
        )
        .await
        {
            tracing::warn!("embedding failed for {}: {}", id, e);
        }
    }

    if let Some(nodes) = &req.nodes {
        for node_req in nodes {
            let node = GraphNode {
                id: Uuid::new_v4().to_string(),
                label: node_req.label.clone(),
                properties: node_req.properties.clone(),
                document_id: Some(id.clone()),
                database_name: database.to_string(),
                collection_name: collection.to_string(),
                created_at: Utc::now(),
            };
            create_node(&state.pool, &node).await.map_err(anyhow::Error::from)?;

            if let Some(edges) = &node_req.edges {
                for er in edges {
                    let targets = find_nodes_by_label(&state.pool, &er.target_label, database, collection)
                        .await
                        .map_err(anyhow::Error::from)?;

                    let target_id = if let Some(t) = targets.first() {
                        t.id.clone()
                    } else {
                        let t = GraphNode {
                            id: Uuid::new_v4().to_string(),
                            label: er.target_label.clone(),
                            properties: serde_json::Value::Object(Default::default()),
                            document_id: None,
                            database_name: database.to_string(),
                            collection_name: collection.to_string(),
                            created_at: Utc::now(),
                        };
                        let tid = t.id.clone();
                        create_node(&state.pool, &t).await.map_err(anyhow::Error::from)?;
                        tid
                    };

                    create_edge(&state.pool, &GraphEdge {
                        id: Uuid::new_v4().to_string(),
                        source_id: node.id.clone(),
                        target_id,
                        relation: er.relation.clone(),
                        weight: er.weight,
                        properties: er.properties.clone(),
                        created_at: Utc::now(),
                    })
                    .await
                    .map_err(anyhow::Error::from)?;
                }
            }
        }
    }

    Ok(Json(Document {
        id,
        content: req.content,
        metadata: req.metadata,
        source: req.source,
        database_name: database.to_string(),
        collection_name: collection.to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }))
}

pub async fn get_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Document>> {
    match fetch_document(&state.pool, &id).await.map_err(anyhow::Error::from)? {
        Some(doc) => Ok(Json(doc)),
        None => Err(ApiError::NotFound(format!("document {} not found", id))),
    }
}

pub async fn delete_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    layerzero_core::embedding::purge_document_vectors(&state.pool, &id)
        .await
        .map_err(anyhow::Error::from)?;
    let r = sqlx::query("DELETE FROM documents WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await
        .map_err(anyhow::Error::from)?;

    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("document {} not found", id)));
    }
    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list_documents(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Document>>> {
    list_documents_in(&state, "default", "default", q).await
}

pub async fn list_documents_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Document>>> {
    list_documents_in(&state, &database, &collection, q).await
}

async fn list_documents_in(
    state: &AppState,
    database: &str,
    collection: &str,
    q: ListQuery,
) -> ApiResult<Json<Vec<Document>>> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    #[derive(sqlx::FromRow)]
    struct Row {
        id: String, content: String, metadata: String, source: Option<String>,
        database_name: String, collection_name: String,
        created_at: String, updated_at: String,
    }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, content, metadata, source, database_name, collection_name, created_at, updated_at
         FROM documents WHERE database_name = ? AND collection_name = ?
         ORDER BY created_at DESC LIMIT ? OFFSET ?"
    )
    .bind(database).bind(collection).bind(limit).bind(offset)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;

    Ok(Json(rows.into_iter().map(|r| Document {
        id: r.id,
        content: r.content,
        metadata: serde_json::from_str(&r.metadata).unwrap_or_default(),
        source: r.source,
        database_name: r.database_name,
        collection_name: r.collection_name,
        created_at: layerzero_core::db::parse_dt(&r.created_at),
        updated_at: layerzero_core::db::parse_dt(&r.updated_at),
    }).collect()))
}

pub async fn get_stats(State(state): State<AppState>) -> ApiResult<Json<Stats>> {
    get_stats_in(&state, None, None).await
}

pub async fn get_stats_scoped(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
) -> ApiResult<Json<Stats>> {
    get_stats_in(&state, Some(&database), Some(&collection)).await
}

async fn get_stats_in(
    state: &AppState,
    database: Option<&str>,
    collection: Option<&str>,
) -> ApiResult<Json<Stats>> {
    let (doc_count, emb_count, node_count) = match (database, collection) {
        (Some(db), Some(col)) => {
            let doc: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM documents WHERE database_name = ? AND collection_name = ?"
            )
            .bind(db).bind(col)
            .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;

            let emb: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM chunks WHERE database_name = ? AND collection_name = ?"
            )
            .bind(db).bind(col)
            .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;

            let node: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM graph_nodes WHERE database_name = ? AND collection_name = ?"
            )
            .bind(db).bind(col)
            .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;

            (doc, emb, node)
        }
        _ => {
            let doc: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
                .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;
            let emb: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chunks")
                .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;
            let node: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_nodes")
                .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;
            (doc, emb, node)
        }
    };

    let edge_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_edges")
        .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;
    let model_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM models")
        .fetch_one(&state.pool).await.map_err(anyhow::Error::from)?;
    let db_size: i64 = sqlx::query_scalar(
        "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()"
    )
    .fetch_optional(&state.pool).await.map_err(anyhow::Error::from)?
    .unwrap_or(0);

    Ok(Json(Stats {
        document_count: doc_count,
        embedding_count: emb_count,
        node_count,
        edge_count,
        model_count,
        db_size_bytes: db_size,
    }))
}
