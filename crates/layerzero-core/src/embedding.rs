use anyhow::Result;
use sqlx::SqlitePool;
use tracing::debug;
use uuid::Uuid;

use crate::chunk::chunk_text;
use crate::db::{now_str, serialize_embedding};
use crate::llm::LlmClient;
use crate::types::{Document, SearchResult};

/// Chunk a document, embed each chunk, and store the vectors in the sqlite-vec
/// index. Replaces any existing chunks/vectors for the document so re-ingest is
/// idempotent.
#[allow(clippy::too_many_arguments)]
pub async fn embed_document(
    pool: &SqlitePool,
    llm: &LlmClient,
    document_id: &str,
    content: &str,
    model: &str,
    database_name: &str,
    collection_name: &str,
    chunk_size: usize,
    chunk_overlap: usize,
) -> Result<usize> {
    purge_document_vectors(pool, document_id).await?;

    let chunks = chunk_text(content, chunk_size, chunk_overlap);
    let now = now_str();
    let mut stored = 0usize;

    for (idx, chunk) in chunks.iter().enumerate() {
        let chunk_id = Uuid::new_v4().to_string();

        let rowid: i64 = sqlx::query_scalar(
            "INSERT INTO chunks (id, document_id, chunk_index, content, database_name, collection_name, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING rowid"
        )
        .bind(&chunk_id)
        .bind(document_id)
        .bind(idx as i64)
        .bind(chunk)
        .bind(database_name)
        .bind(collection_name)
        .bind(&now)
        .fetch_one(pool)
        .await?;

        let embedding = llm.embed_one(chunk, model).await?;
        let blob = serialize_embedding(&embedding);

        sqlx::query("INSERT INTO vec_chunks (rowid, embedding) VALUES (?, ?)")
            .bind(rowid)
            .bind(&blob)
            .execute(pool)
            .await?;

        stored += 1;
    }

    debug!("embedded document {} into {} chunks", document_id, stored);
    Ok(stored)
}

/// Remove all chunks and their vectors for a single document.
pub async fn purge_document_vectors(pool: &SqlitePool, document_id: &str) -> Result<()> {
    sqlx::query(
        "DELETE FROM vec_chunks WHERE rowid IN (SELECT rowid FROM chunks WHERE document_id = ?)",
    )
    .bind(document_id)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM chunks WHERE document_id = ?")
        .bind(document_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove chunks/vectors for an entire collection (or whole database when
/// `collection_name` is None). Call before deleting the documents themselves.
pub async fn purge_collection_vectors(
    pool: &SqlitePool,
    database_name: &str,
    collection_name: Option<&str>,
) -> Result<()> {
    match collection_name {
        Some(col) => {
            sqlx::query(
                "DELETE FROM vec_chunks WHERE rowid IN (SELECT rowid FROM chunks WHERE database_name = ? AND collection_name = ?)",
            )
            .bind(database_name)
            .bind(col)
            .execute(pool)
            .await?;
            sqlx::query("DELETE FROM chunks WHERE database_name = ? AND collection_name = ?")
                .bind(database_name)
                .bind(col)
                .execute(pool)
                .await?;
        }
        None => {
            sqlx::query(
                "DELETE FROM vec_chunks WHERE rowid IN (SELECT rowid FROM chunks WHERE database_name = ?)",
            )
            .bind(database_name)
            .execute(pool)
            .await?;
            sqlx::query("DELETE FROM chunks WHERE database_name = ?")
                .bind(database_name)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn search_similar(
    pool: &SqlitePool,
    llm: &LlmClient,
    query: &str,
    model: &str,
    limit: usize,
    threshold: f32,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<SearchResult>> {
    let query_emb = llm.embed_one(query, model).await?;
    search_by_embedding(pool, &query_emb, model, limit, threshold, database_name, collection_name).await
}

#[allow(unused_variables)]
pub async fn search_by_embedding(
    pool: &SqlitePool,
    query_emb: &[f32],
    model: &str,
    limit: usize,
    threshold: f32,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<SearchResult>> {
    if limit == 0 {
        return Ok(vec![]);
    }
    let blob = serialize_embedding(query_emb);
    // Over-fetch from the global KNN, then filter to this collection. Keeps recall
    // high without per-collection index tables.
    let overfetch = (limit.saturating_mul(50)).clamp(50, 5000) as i64;

    #[derive(sqlx::FromRow)]
    struct Hit {
        document_id: String,
        chunk_content: String,
        distance: f64,
    }

    let hits: Vec<Hit> = sqlx::query_as(
        r#"SELECT c.document_id AS document_id, c.content AS chunk_content, knn.distance AS distance
           FROM (
               SELECT rowid, distance FROM vec_chunks
               WHERE embedding MATCH ? ORDER BY distance LIMIT ?
           ) knn
           JOIN chunks c ON c.rowid = knn.rowid
           WHERE c.database_name = ? AND c.collection_name = ?
           ORDER BY knn.distance"#,
    )
    .bind(&blob)
    .bind(overfetch)
    .bind(database_name)
    .bind(collection_name)
    .fetch_all(pool)
    .await?;

    // Collapse chunk hits to their parent documents, keeping the best chunk.
    let mut seen: std::collections::HashMap<String, (f32, String)> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for h in hits {
        let score = 1.0 - h.distance as f32;
        if score < threshold {
            continue;
        }
        match seen.get(&h.document_id) {
            Some((best, _)) if *best >= score => {}
            _ => {
                if !seen.contains_key(&h.document_id) {
                    order.push(h.document_id.clone());
                }
                seen.insert(h.document_id.clone(), (score, h.chunk_content));
            }
        }
    }

    let mut results = Vec::new();
    for doc_id in order.into_iter().take(limit) {
        let (score, chunk) = seen.remove(&doc_id).unwrap();
        if let Some(doc) = fetch_document(pool, &doc_id).await? {
            results.push(SearchResult {
                document: doc,
                score,
                rerank_score: None,
                matched_chunk: Some(chunk),
            });
        }
    }

    Ok(results)
}

pub async fn fts_search(
    pool: &SqlitePool,
    query: &str,
    limit: usize,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<SearchResult>> {
    #[derive(sqlx::FromRow)]
    struct FtsRow {
        id: String,
        content: String,
        metadata: String,
        source: Option<String>,
        database_name: String,
        collection_name: String,
        created_at: String,
        updated_at: String,
        score: Option<f64>,
    }

    let rows: Vec<FtsRow> = sqlx::query_as(
        r#"SELECT d.id, d.content, d.metadata, d.source, d.database_name, d.collection_name,
                  d.created_at, d.updated_at,
                  bm25(documents_fts) as score
           FROM documents d
           JOIN documents_fts ON documents_fts.rowid = d.rowid
           WHERE documents_fts MATCH ?
             AND d.database_name = ? AND d.collection_name = ?
           ORDER BY score
           LIMIT ?"#,
    )
    .bind(query)
    .bind(database_name)
    .bind(collection_name)
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
                database_name: row.database_name,
                collection_name: row.collection_name,
                created_at: crate::db::parse_dt(&row.created_at),
                updated_at: crate::db::parse_dt(&row.updated_at),
            },
            score: -(row.score.unwrap_or(0.0) as f32),
            rerank_score: None,
            matched_chunk: None,
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
        database_name: String,
        collection_name: String,
        created_at: String,
        updated_at: String,
    }

    let row: Option<DocRow> = sqlx::query_as(
        "SELECT id, content, metadata, source, database_name, collection_name, created_at, updated_at FROM documents WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Document {
        id: r.id,
        content: r.content,
        metadata: serde_json::from_str(&r.metadata).unwrap_or_default(),
        source: r.source,
        database_name: r.database_name,
        collection_name: r.collection_name,
        created_at: crate::db::parse_dt(&r.created_at),
        updated_at: crate::db::parse_dt(&r.updated_at),
    }))
}
