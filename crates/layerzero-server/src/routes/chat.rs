use axum::{body::Body, extract::State, response::{IntoResponse, Response}, Json};
use futures::StreamExt;
use layerzero_core::types::ChatCompletionRequest;

use crate::routes::ApiResult;
use crate::state::AppState;

pub async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionRequest>,
) -> ApiResult<Response> {
    let stream = req.stream.unwrap_or(false);

    if stream {
        let resp = state.llm.chat_stream(&req).await.map_err(anyhow::Error::from)?;
        let bytes_stream = resp.bytes_stream().map(|r| {
            r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        });
        Ok(Response::builder()
            .header("Content-Type", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("X-Accel-Buffering", "no")
            .body(Body::from_stream(bytes_stream))
            .unwrap())
    } else {
        let resp = state.llm.chat(&req).await.map_err(anyhow::Error::from)?;
        Ok(Json(resp).into_response())
    }
}
