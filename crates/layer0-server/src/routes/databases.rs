use axum::{extract::{Path, State}, Json};
use layer0_core::{
    database::{
        create_collection, create_database, delete_collection, delete_database,
        get_collection, get_database, list_collections, list_databases,
    },
    types::{Collection, CreateCollectionRequest, CreateDatabaseRequest, Database},
};

use crate::routes::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn list_databases_route(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Database>>> {
    Ok(Json(list_databases(&state.pool).await.map_err(anyhow::Error::from)?))
}

pub async fn create_database_route(
    State(state): State<AppState>,
    Json(req): Json<CreateDatabaseRequest>,
) -> ApiResult<Json<Database>> {
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("database name cannot be empty".into()));
    }
    let db = create_database(&state.pool, &state.config, &req.name, req.description.as_deref())
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(db))
}

pub async fn get_database_route(
    State(state): State<AppState>,
    Path(database): Path<String>,
) -> ApiResult<Json<Database>> {
    match get_database(&state.pool, &database).await.map_err(anyhow::Error::from)? {
        Some(db) => Ok(Json(db)),
        None => Err(ApiError::NotFound(format!("database '{}' not found", database))),
    }
}

pub async fn delete_database_route(
    State(state): State<AppState>,
    Path(database): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    delete_database(&state.pool, &state.config, &database)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(serde_json::json!({ "deleted": true, "database": database })))
}

pub async fn list_collections_route(
    State(state): State<AppState>,
    Path(database): Path<String>,
) -> ApiResult<Json<Vec<Collection>>> {
    Ok(Json(
        list_collections(&state.pool, &database)
            .await
            .map_err(anyhow::Error::from)?,
    ))
}

pub async fn create_collection_route(
    State(state): State<AppState>,
    Path(database): Path<String>,
    Json(req): Json<CreateCollectionRequest>,
) -> ApiResult<Json<Collection>> {
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("collection name cannot be empty".into()));
    }
    let col = create_collection(&state.pool, &database, &req.name, req.description.as_deref())
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(col))
}

pub async fn get_collection_route(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
) -> ApiResult<Json<Collection>> {
    match get_collection(&state.pool, &database, &collection)
        .await
        .map_err(anyhow::Error::from)?
    {
        Some(col) => Ok(Json(col)),
        None => Err(ApiError::NotFound(format!(
            "collection '{}/{}' not found",
            database, collection
        ))),
    }
}

pub async fn delete_collection_route(
    State(state): State<AppState>,
    Path((database, collection)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    delete_collection(&state.pool, &database, &collection)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(serde_json::json!({ "deleted": true, "database": database, "collection": collection })))
}
