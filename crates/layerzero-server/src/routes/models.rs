use axum::{extract::{Path, State}, Json};
use layerzero_core::{
    installer::{download_hf_model, install_llama_cpp, list_installed_models},
    types::{DownloadModelRequest, ModelInfo},
};

use crate::routes::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn list_models(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        name: String,
        model_type: String,
        path: Option<String>,
        context_length: Option<i64>,
        dimensions: Option<i64>,
        metadata: String,
        created_at: String,
    }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT name, model_type, path, context_length, dimensions, metadata, created_at FROM models ORDER BY created_at DESC"
    )
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;

    let db_models: Vec<ModelInfo> = rows.into_iter().map(|r| ModelInfo {
        name: r.name,
        model_type: r.model_type,
        path: r.path,
        context_length: r.context_length,
        dimensions: r.dimensions,
        metadata: serde_json::from_str(&r.metadata).unwrap_or_default(),
        created_at: layerzero_core::db::parse_dt(&r.created_at),
    }).collect();

    let remote = state.llm.list_models().await.unwrap_or_default();
    let installed = list_installed_models(&state.config.installer.models_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
        .collect::<Vec<_>>();

    Ok(Json(serde_json::json!({
        "registered": db_models,
        "backend": remote,
        "installed_files": installed,
        "models_dir": state.config.installer.models_dir.display().to_string(),
    })))
}

pub async fn download_model(
    State(state): State<AppState>,
    Json(req): Json<DownloadModelRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let token = req.hf_token.as_deref().or(state.config.installer.hf_token.as_deref());
    let path = download_hf_model(&state.config.installer, &req.repo, &req.filename, token)
        .await
        .map_err(anyhow::Error::from)?;

    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let path_str = path.to_string_lossy().to_string();
    let now = layerzero_core::db::now_str();

    sqlx::query(
        "INSERT OR REPLACE INTO models (name, model_type, path, metadata, created_at) VALUES (?, ?, ?, '{}', ?)"
    )
    .bind(&name)
    .bind(&req.model_type)
    .bind(&path_str)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;

    Ok(Json(serde_json::json!({
        "name": name,
        "path": path_str,
        "model_type": req.model_type,
        "repo": req.repo,
        "filename": req.filename,
    })))
}

pub async fn delete_model(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct Row { path: Option<String> }

    let row: Option<Row> = sqlx::query_as("SELECT path FROM models WHERE name = ?")
        .bind(&name)
        .fetch_optional(&state.pool)
        .await
        .map_err(anyhow::Error::from)?;

    match row {
        None => Err(ApiError::NotFound(format!("model {} not found", name))),
        Some(r) => {
            if let Some(p) = r.path { let _ = std::fs::remove_file(&p); }
            sqlx::query("DELETE FROM models WHERE name = ?")
                .bind(&name)
                .execute(&state.pool)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(Json(serde_json::json!({ "deleted": true, "name": name })))
        }
    }
}

pub async fn install_llama(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let dir = install_llama_cpp(&state.config.installer)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(serde_json::json!({ "installed": true, "bin_dir": dir.display().to_string() })))
}
