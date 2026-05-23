use anyhow::Result;
use layerzero_core::{
    config::Config,
    database::ensure_collection,
    db::connect,
    embedding::{embed_document, search_similar},
    graph::bfs_traverse,
    llm::LlmClient,
    rag::rag_query,
    types::RagRequest,
};
use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

pub struct ToolContext {
    pub pool: SqlitePool,
    pub config: Config,
    pub llm: LlmClient,
}

impl ToolContext {
    pub async fn new(config: Config) -> Result<Self> {
        let pool = connect(&config).await?;
        let llm = LlmClient::new(&config.llm)?;
        Ok(Self { pool, config, llm })
    }
}

fn get_db(args: &Value) -> &str {
    args["database"].as_str().unwrap_or("default")
}

fn get_col(args: &Value) -> &str {
    args["collection"].as_str().unwrap_or("default")
}

pub async fn store_memory(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let content = args["content"].as_str().ok_or_else(|| anyhow::anyhow!("content required"))?;
    let source = args["source"].as_str().map(String::from);
    let metadata = args.get("metadata").cloned().unwrap_or_default();
    let meta_str = serde_json::to_string(&metadata)?;
    let database = get_db(args);
    let collection = get_col(args);

    let id = Uuid::new_v4().to_string();
    let now = layerzero_core::db::now_str();

    ensure_collection(&ctx.pool, database, collection).await?;

    sqlx::query(
        "INSERT INTO documents (id, content, metadata, source, database_name, collection_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(content)
    .bind(&meta_str)
    .bind(&source)
    .bind(database)
    .bind(collection)
    .bind(&now)
    .bind(&now)
    .execute(&ctx.pool)
    .await?;

    if let Err(e) = embed_document(&ctx.pool, &ctx.llm, &id, content, &ctx.config.llm.embedding_model).await {
        tracing::warn!("embedding failed: {}", e);
    }

    Ok(serde_json::json!({ "id": id, "stored": true, "database": database, "collection": collection }))
}

pub async fn search_memory(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let query = args["query"].as_str().ok_or_else(|| anyhow::anyhow!("query required"))?;
    let limit = args["limit"].as_u64().unwrap_or(5) as usize;
    let database = get_db(args);
    let collection = get_col(args);

    let results = search_similar(
        &ctx.pool, &ctx.llm, query, &ctx.config.llm.embedding_model, limit, 0.0, database, collection,
    )
    .await?;

    Ok(serde_json::json!({
        "query": query,
        "database": database,
        "collection": collection,
        "count": results.len(),
        "results": results.iter().map(|r| serde_json::json!({
            "id": r.document.id,
            "content": r.document.content,
            "source": r.document.source,
            "score": r.score,
            "metadata": r.document.metadata,
        })).collect::<Vec<_>>()
    }))
}

pub async fn rag_tool(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let query = args["query"].as_str().ok_or_else(|| anyhow::anyhow!("query required"))?;
    let limit = args["limit"].as_u64().unwrap_or(5) as usize;
    let system_prompt = args["system_prompt"].as_str().map(String::from);
    let database = get_db(args).to_string();
    let collection = get_col(args).to_string();

    let resp = rag_query(
        &ctx.pool, &ctx.llm,
        &RagRequest {
            query: query.to_string(),
            limit,
            system_prompt,
            model: None,
            embedding_model: None,
            use_graph: false,
            rerank: true,
            stream: false,
            database,
            collection,
        },
        &ctx.config.llm.embedding_model,
        &ctx.config.llm.chat_model,
    )
    .await?;

    Ok(serde_json::json!({
        "answer": resp.answer,
        "source_count": resp.sources.len(),
        "sources": resp.sources.iter().map(|r| serde_json::json!({
            "id": r.document.id,
            "content": r.document.content,
            "source": r.document.source,
            "score": r.score,
        })).collect::<Vec<_>>()
    }))
}

pub async fn get_document_tool(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let id = args["id"].as_str().ok_or_else(|| anyhow::anyhow!("id required"))?;

    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        content: String,
        metadata: String,
        source: Option<String>,
        database_name: String,
        collection_name: String,
        created_at: String,
    }

    let row: Option<Row> = sqlx::query_as(
        "SELECT id, content, metadata, source, database_name, collection_name, created_at FROM documents WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(&ctx.pool)
    .await?;

    match row {
        Some(r) => Ok(serde_json::json!({
            "id": r.id, "content": r.content,
            "metadata": serde_json::from_str::<Value>(&r.metadata).unwrap_or_default(),
            "source": r.source,
            "database": r.database_name,
            "collection": r.collection_name,
            "created_at": r.created_at,
        })),
        None => Ok(serde_json::json!({ "error": "not found", "id": id })),
    }
}

pub async fn delete_memory_tool(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let id = args["id"].as_str().ok_or_else(|| anyhow::anyhow!("id required"))?;
    let r = sqlx::query("DELETE FROM documents WHERE id = ?")
        .bind(id)
        .execute(&ctx.pool)
        .await?;
    Ok(serde_json::json!({ "deleted": r.rows_affected() > 0, "id": id }))
}

pub async fn graph_query_tool(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let database = get_db(args);
    let collection = get_col(args);

    let start_id = if let Some(id) = args["start_node_id"].as_str() {
        id.to_string()
    } else if let Some(label) = args["start_label"].as_str() {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM graph_nodes WHERE label = ? AND database_name = ? AND collection_name = ? LIMIT 1"
        )
        .bind(label)
        .bind(database)
        .bind(collection)
        .fetch_optional(&ctx.pool)
        .await?;
        row.map(|(id,)| id)
            .ok_or_else(|| anyhow::anyhow!("no node with that label"))?
    } else {
        return Err(anyhow::anyhow!("start_node_id or start_label required"));
    };

    let depth = args["depth"].as_u64().unwrap_or(2) as usize;
    let relation = args["relation"].as_str();
    let result = bfs_traverse(&ctx.pool, &start_id, depth, relation, "both", database, collection).await?;

    Ok(serde_json::json!({
        "nodes": result.nodes.len(),
        "edges": result.edges.len(),
        "documents": result.documents.iter().map(|d| serde_json::json!({
            "id": d.id, "content": d.content, "source": d.source
        })).collect::<Vec<_>>()
    }))
}

pub async fn memory_stats_tool(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let database = get_db(args);
    let collection = get_col(args);
    let scoped = database != "default" || collection != "default";

    let (docs, embs, nodes) = if scoped {
        let d: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM documents WHERE database_name = ? AND collection_name = ?"
        ).bind(database).bind(collection).fetch_one(&ctx.pool).await?;
        let e: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM embeddings e JOIN documents d ON e.document_id = d.id WHERE d.database_name = ? AND d.collection_name = ?"
        ).bind(database).bind(collection).fetch_one(&ctx.pool).await?;
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM graph_nodes WHERE database_name = ? AND collection_name = ?"
        ).bind(database).bind(collection).fetch_one(&ctx.pool).await?;
        (d, e, n)
    } else {
        let d: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents").fetch_one(&ctx.pool).await?;
        let e: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM embeddings").fetch_one(&ctx.pool).await?;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_nodes").fetch_one(&ctx.pool).await?;
        (d, e, n)
    };

    let edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_edges").fetch_one(&ctx.pool).await?;

    Ok(serde_json::json!({
        "database": database,
        "collection": collection,
        "documents": docs, "embeddings": embs,
        "graph_nodes": nodes, "graph_edges": edges,
        "embedding_model": ctx.config.llm.embedding_model,
        "chat_model": ctx.config.llm.chat_model,
        "llm_url": ctx.config.llm.base_url,
    }))
}
