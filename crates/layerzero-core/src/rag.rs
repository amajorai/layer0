use anyhow::Result;
use tracing::debug;

use crate::embedding::{fts_search, search_similar};
use crate::graph::bfs_traverse;
use crate::llm::LlmClient;
use crate::rerank::{reciprocal_rank_fusion, rerank};
use crate::types::{ChatCompletionRequest, ChatMessage, RagRequest, RagResponse, SearchResult};
use sqlx::SqlitePool;

pub async fn rag_query(
    pool: &SqlitePool,
    llm: &LlmClient,
    request: &RagRequest,
    embedding_model: &str,
    chat_model: &str,
) -> Result<RagResponse> {
    debug!("RAG: {}", request.query);

    let db = &request.database;
    let col = &request.collection;

    let vector_results = search_similar(
        pool, llm, &request.query, embedding_model, request.limit * 3, 0.0, db, col,
    )
    .await?;

    let fts_results = fts_search(pool, &request.query, request.limit * 2, db, col)
        .await
        .unwrap_or_default();

    let mut sources = if !fts_results.is_empty() {
        reciprocal_rank_fusion(vec![vector_results, fts_results], 60.0)
    } else {
        vector_results
    };

    if request.use_graph && !sources.is_empty() {
        let top_id = sources[0].document.id.clone();
        if let Ok(extras) = expand_with_graph(pool, &top_id, 1, db, col).await {
            let existing_ids: std::collections::HashSet<String> =
                sources.iter().map(|r| r.document.id.clone()).collect();
            for r in extras {
                if !existing_ids.contains(&r.document.id) {
                    sources.push(r);
                }
            }
        }
    }

    if request.rerank && sources.len() > 1 {
        sources = rerank(llm, &request.query, sources, embedding_model).await;
    }

    sources.truncate(request.limit);

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
            format!("[{}] (source: {}, score: {:.3})\n{}", i + 1, src, r.score, r.document.content)
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

async fn expand_with_graph(
    pool: &SqlitePool,
    document_id: &str,
    depth: usize,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<SearchResult>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM graph_nodes WHERE document_id = ? AND database_name = ? AND collection_name = ? LIMIT 1",
    )
    .bind(document_id)
    .bind(database_name)
    .bind(collection_name)
    .fetch_optional(pool)
    .await?;

    if let Some((node_id,)) = row {
        let g = bfs_traverse(pool, &node_id, depth, None, "both", database_name, collection_name).await?;
        Ok(g.documents
            .into_iter()
            .filter(|d| d.id != document_id)
            .map(|d| SearchResult { document: d, score: 0.5, rerank_score: None })
            .collect())
    } else {
        Ok(vec![])
    }
}
