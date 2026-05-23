use anyhow::Result;
use sqlx::SqlitePool;
use tracing::debug;

use crate::llm::LlmClient;
use crate::retrieval::{retrieve, RagMode};
use crate::types::{ChatCompletionRequest, ChatMessage, RagRequest, RagResponse, SearchResult};

pub async fn rag_query(
    pool: &SqlitePool,
    llm: &LlmClient,
    request: &RagRequest,
    embedding_model: &str,
    chat_model: &str,
) -> Result<RagResponse> {
    debug!("RAG ({:?}): {}", request.mode, request.query);

    let mode = RagMode::parse(request.mode.as_deref().unwrap_or("hybrid"));
    let sources = retrieve(
        pool,
        llm,
        &request.query,
        embedding_model,
        request.limit,
        mode,
        request.rerank,
        &request.database,
        &request.collection,
    )
    .await?;

    let context = build_context(&sources);

    let system = request.system_prompt.clone().unwrap_or_else(|| {
        "You are a helpful assistant. Use the provided context to answer the user's question. \
         If the context does not contain enough information, say so clearly."
            .to_string()
    });

    let messages = vec![
        ChatMessage { role: "system".to_string(), content: system, name: None },
        ChatMessage {
            role: "user".to_string(),
            content: format!("Context:\n{}\n\nQuestion: {}", context, request.query),
            name: None,
        },
    ];

    let chat_req = ChatCompletionRequest {
        model: request.model.clone().unwrap_or_else(|| chat_model.to_string()),
        messages,
        temperature: Some(0.7),
        max_tokens: Some(2048),
        stream: Some(false),
        top_p: None,
        stop: None,
    };

    let response = llm.chat(&chat_req).await?;
    let answer = response.choices.first().map(|c| c.message.content.clone()).unwrap_or_default();

    Ok(RagResponse { answer, sources, usage: response.usage })
}

fn build_context(results: &[SearchResult]) -> String {
    results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let src = r.document.source.as_deref().unwrap_or("unknown");
            let text = r.matched_chunk.as_deref().unwrap_or(&r.document.content);
            format!("[{}] (source: {}, score: {:.3})\n{}", i + 1, src, r.score, text)
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}
