use anyhow::Result;
use sqlx::SqlitePool;
use std::collections::HashSet;

use crate::embedding::{fts_search, search_similar};
use crate::graph::bfs_traverse;
use crate::llm::LlmClient;
use crate::rerank::{reciprocal_rank_fusion, rerank};
use crate::types::SearchResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RagMode {
    Hybrid,
    Vector,
    Graph,
}

impl RagMode {
    pub fn parse(s: &str) -> RagMode {
        match s.trim().to_ascii_lowercase().as_str() {
            "vector" => RagMode::Vector,
            "graph" => RagMode::Graph,
            _ => RagMode::Hybrid,
        }
    }
}

/// Unified retrieval used by both search and RAG. `mode` selects the strategy;
/// `rerank_final` applies a reranking pass to the result.
#[allow(clippy::too_many_arguments)]
pub async fn retrieve(
    pool: &SqlitePool,
    llm: &LlmClient,
    query: &str,
    embedding_model: &str,
    limit: usize,
    mode: RagMode,
    rerank_final: bool,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<SearchResult>> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(vec![]);
    }

    let mut results = match mode {
        RagMode::Vector => fused(pool, llm, query, embedding_model, limit, database_name, collection_name).await?,

        RagMode::Hybrid => {
            let mut base = fused(pool, llm, query, embedding_model, limit, database_name, collection_name).await?;
            if let Some(top) = base.first().map(|r| r.document.id.clone()) {
                let extras = expand_with_graph(pool, &top, 1, database_name, collection_name).await.unwrap_or_default();
                merge(&mut base, extras);
            }
            base
        }

        RagMode::Graph => {
            // Vector-seed a few entry points, then return their graph neighborhood.
            let seeds = search_similar(pool, llm, query, embedding_model, limit, 0.0, database_name, collection_name).await?;
            let mut out: Vec<SearchResult> = Vec::new();
            for seed in seeds.iter().take(3) {
                let extras = expand_with_graph(pool, &seed.document.id, 2, database_name, collection_name).await.unwrap_or_default();
                merge(&mut out, extras);
            }
            // Include the seeds themselves so a sparse graph still returns something.
            merge(&mut out, seeds);
            out
        }
    };

    if rerank_final && results.len() > 1 {
        results = rerank(llm, query, results, embedding_model).await;
    }

    results.truncate(limit);
    Ok(results)
}

/// Vector + FTS fused with Reciprocal Rank Fusion (no graph, no rerank).
async fn fused(
    pool: &SqlitePool,
    llm: &LlmClient,
    query: &str,
    embedding_model: &str,
    limit: usize,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<SearchResult>> {
    let vector = search_similar(pool, llm, query, embedding_model, limit * 3, 0.0, database_name, collection_name).await?;
    let fts = fts_search(pool, query, limit * 2, database_name, collection_name).await.unwrap_or_default();
    Ok(if fts.is_empty() {
        vector
    } else {
        reciprocal_rank_fusion(vec![vector, fts], 60.0)
    })
}

/// Append results whose document isn't already present.
fn merge(into: &mut Vec<SearchResult>, extras: Vec<SearchResult>) {
    let seen: HashSet<String> = into.iter().map(|r| r.document.id.clone()).collect();
    let mut seen = seen;
    for r in extras {
        if seen.insert(r.document.id.clone()) {
            into.push(r);
        }
    }
}

/// Documents reachable in the graph from the same node(s) as `document_id`.
pub async fn expand_with_graph(
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

    let Some((node_id,)) = row else {
        return Ok(vec![]);
    };

    let g = bfs_traverse(pool, &node_id, depth, None, "both", database_name, collection_name).await?;
    Ok(g.documents
        .into_iter()
        .filter(|d| d.id != document_id)
        .map(|d| SearchResult { document: d, score: 0.5, rerank_score: None, matched_chunk: None })
        .collect())
}
