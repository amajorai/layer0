use std::collections::HashMap;

use crate::llm::LlmClient;
use crate::types::SearchResult;

// Returns results (possibly reranked). On error, returns originals unchanged.
pub async fn rerank(
    llm: &LlmClient,
    query: &str,
    mut results: Vec<SearchResult>,
    model: &str,
) -> Vec<SearchResult> {
    if results.is_empty() {
        return results;
    }

    let docs: Vec<String> = results.iter().map(|r| r.document.content.clone()).collect();
    let scores = match llm.rerank_with_llm(query, &docs, model).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("rerank failed: {}", e);
            return results;
        }
    };

    for (r, s) in results.iter_mut().zip(scores) {
        r.rerank_score = Some(s);
    }

    results.sort_by(|a, b| {
        let a_s = a.rerank_score.unwrap_or(a.score);
        let b_s = b.rerank_score.unwrap_or(b.score);
        b_s.partial_cmp(&a_s).unwrap_or(std::cmp::Ordering::Equal)
    });

    results
}

/// Reciprocal Rank Fusion for merging multiple result lists.
pub fn reciprocal_rank_fusion(result_lists: Vec<Vec<SearchResult>>, k: f32) -> Vec<SearchResult> {
    let mut scores: HashMap<String, (SearchResult, f32)> = HashMap::new();

    for list in result_lists {
        for (rank, result) in list.into_iter().enumerate() {
            let rrf = 1.0 / (k + rank as f32 + 1.0);
            let entry = scores.entry(result.document.id.clone()).or_insert((result, 0.0));
            entry.1 += rrf;
        }
    }

    let mut merged: Vec<SearchResult> = scores
        .into_values()
        .map(|(mut r, s)| {
            r.score = s;
            r
        })
        .collect();

    merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    merged
}

pub fn lexical_score(query: &str, document: &str) -> f32 {
    let query_words: std::collections::HashSet<&str> = query.split_whitespace().collect();
    let doc_words: Vec<&str> = document.split_whitespace().collect();
    let doc_len = doc_words.len() as f32;

    if query_words.is_empty() || doc_len == 0.0 {
        return 0.0;
    }

    let k1 = 1.5_f32;
    let b = 0.75_f32;
    let avg_len = 100.0_f32;
    let mut score = 0.0_f32;

    for term in &query_words {
        let tf = doc_words.iter().filter(|w| w.eq_ignore_ascii_case(term)).count() as f32;
        score += (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * doc_len / avg_len));
    }
    score
}
