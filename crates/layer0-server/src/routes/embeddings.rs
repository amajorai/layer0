use axum::{extract::State, Json};
use layer0_core::types::{EmbeddingData, EmbeddingRequest, EmbeddingResponse, EmbeddingUsage};

use crate::routes::ApiResult;
use crate::state::AppState;

pub async fn create_embeddings(
    State(state): State<AppState>,
    Json(req): Json<EmbeddingRequest>,
) -> ApiResult<Json<EmbeddingResponse>> {
    let texts = req.input.as_slice();
    let token_count: u32 = texts.iter().map(|t| (t.len() / 4) as u32).sum();

    let model = if req.model.is_empty() || req.model == "local" {
        state.embedding_model().to_string()
    } else {
        req.model.clone()
    };

    let embeddings = state.llm.embed(&texts, &model).await.map_err(anyhow::Error::from)?;

    Ok(Json(EmbeddingResponse {
        object: "list".to_string(),
        data: embeddings
            .into_iter()
            .enumerate()
            .map(|(i, emb)| EmbeddingData {
                object: "embedding".to_string(),
                embedding: emb,
                index: i,
            })
            .collect(),
        model,
        usage: EmbeddingUsage { prompt_tokens: token_count, total_tokens: token_count },
    }))
}
