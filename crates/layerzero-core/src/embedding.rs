use anyhow::Result;
use sqlx::SqlitePool;
use tracing::debug;

use crate::db::{cosine_similarity, deserialize_embedding, now_str, serialize_embedding};
use crate::llm::LlmClient;
use crate::types::{Document, SearchResult};

pub async fn embed_document(
    pool: &SqlitePool,
    llm: &LlmClient,
    document_id: &str,
    content: &str,
    model: &str,
) -> Result<()> {
    let embedding = llm.embed_one(content, model).await?;
    let blob = serialize_embedding(&embedding);
    let dims = embedding.len() as i64;
    let now = now_str();

    sqlx::query(
        "INSERT INTO embeddings (document_id, model, data, dimensions, created_at) VALUES (?, ?, ?, ?, ?)"
    )
    .bind(document_id)
    .bind(model)
    .bind(&blob)
    .bind(dims)
    .bind(&now)
    .execute(pool)
    .await?;

    debug!("embedded document {} dims={}", document_id, dims);
    Ok(())
}

pub async fn search_similar(
    pool: &SqlitePool,
    llm: &LlmClient,
    query: &str,
    model: &str,
    limit: usize,
    threshold: f32,
) -> Result<Vec<SearchResult>> {
    let query_emb = llm.embed_one(query, model).await?;
    search_by_embedding(pool, &query_emb, model, limit, threshold).await
}

pub async fn search_by_embedding(
    pool: &SqlitePool,
    query_emb: &[f32],
    model: &str,
    limit: usize,
    threshold: f32,
) -> Result<Vec<SearchResult>> {
    #[derive(sqlx::FromRow)]
    struct EmbRow {
        document_id: String,
        data: Vec<u8>,
    }

    let rows: Vec<EmbRow> = sqlx::query_as(
        "SELECT document_id, data FROM embeddings WHERE model = ?"
    )
    .bind(model)
    .fetch_all(pool)
    .await?;

    let mut scored: Vec<(String, f32)> = rows
        .into_iter()
        .filter_map(|row| {
            let emb = deserialize_embedding(&row.data);
            let score = cosine_similarity(query_emb, &emb);
            if score >= threshold { Some((row.document_id, score)) } else { None }
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);

    let mut results = Vec::with_capacity(scored.len());
    for (doc_id, score) in &scored {
        let row = fetch_document(pool, doc_id).await?;
        if let Some(doc) = row {
            results.push(SearchResult { document: doc, score: *score, rerank_score: None });
        }
    }

    Ok(results)
}

pub async fn fts_search(
    pool: &SqlitePool,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    #[derive(sqlx::FromRow)]
    struct FtsRow {
        id: String,
        content: String,
        metadata: String,
        source: Option<String>,
        created_at: String,
        updated_at: String,
        score: Option<f64>,
    }

    let rows: Vec<FtsRow> = sqlx::query_as(
        r#"SELECT d.id, d.content, d.metadata, d.source, d.created_at, d.updated_at,
                  bm25(documents_fts) as score
           FROM documents d
           JOIN documents_fts ON documents_fts.rowid = d.rowid
           WHERE documents_fts MATCH ?
           ORDER BY score
           LIMIT ?"#,
    )
    .bind(query)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| SearchResult {
            document: Document {
                id: row.id,
                content: row.content,
                metadata: serde_json::from_str(&row.metadata).unwrap_or_default(),
                source: row.source,
                created_at: crate::db::parse_dt(&row.created_at),
                updated_at: crate::db::parse_dt(&row.updated_at),
            },
            score: -(row.score.unwrap_or(0.0) as f32),
            rerank_score: None,
        })
        .collect())
}

pub async fn fetch_document(pool: &SqlitePool, id: &str) -> Result<Option<Document>> {
    #[derive(sqlx::FromRow)]
    struct DocRow {
        id: String,
        content: String,
        metadata: String,
        source: Option<String>,
        created_at: String,
        updated_at: String,
    }

    let row: Option<DocRow> = sqlx::query_as(
        "SELECT id, content, metadata, source, created_at, updated_at FROM documents WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Document {
        id: r.id,
        content: r.content,
        metadata: serde_json::from_str(&r.metadata).unwrap_or_default(),
        source: r.source,
        created_at: crate::db::parse_dt(&r.created_at),
        updated_at: crate::db::parse_dt(&r.updated_at),
    }))
}
