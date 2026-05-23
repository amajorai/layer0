use anyhow::Result;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::db::now_str;
use crate::types::{GraphEdge, GraphNode, GraphSearchResult};

fn parse_node(
    id: String,
    label: String,
    properties: String,
    document_id: Option<String>,
    database_name: String,
    collection_name: String,
    created_at: String,
) -> GraphNode {
    GraphNode {
        id,
        label,
        properties: serde_json::from_str(&properties).unwrap_or_default(),
        document_id,
        database_name,
        collection_name,
        created_at: crate::db::parse_dt(&created_at),
    }
}

fn parse_edge(id: String, source_id: String, target_id: String, relation: String, weight: f64, properties: String, created_at: String) -> GraphEdge {
    GraphEdge {
        id,
        source_id,
        target_id,
        relation,
        weight,
        properties: serde_json::from_str(&properties).unwrap_or_default(),
        created_at: crate::db::parse_dt(&created_at),
    }
}

pub async fn create_node(pool: &SqlitePool, node: &GraphNode) -> Result<()> {
    let props = serde_json::to_string(&node.properties)?;
    let now = now_str();
    sqlx::query(
        "INSERT OR REPLACE INTO graph_nodes (id, label, properties, document_id, database_name, collection_name, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&node.id)
    .bind(&node.label)
    .bind(&props)
    .bind(&node.document_id)
    .bind(&node.database_name)
    .bind(&node.collection_name)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn create_edge(pool: &SqlitePool, edge: &GraphEdge) -> Result<()> {
    let props = serde_json::to_string(&edge.properties)?;
    let now = now_str();
    sqlx::query(
        "INSERT OR REPLACE INTO graph_edges (id, source_id, target_id, relation, weight, properties, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&edge.id)
    .bind(&edge.source_id)
    .bind(&edge.target_id)
    .bind(&edge.relation)
    .bind(edge.weight)
    .bind(&props)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_node(pool: &SqlitePool, node_id: &str) -> Result<Option<GraphNode>> {
    #[derive(sqlx::FromRow)]
    struct Row { id: String, label: String, properties: String, document_id: Option<String>, database_name: String, collection_name: String, created_at: String }

    let r: Option<Row> = sqlx::query_as(
        "SELECT id, label, properties, document_id, database_name, collection_name, created_at FROM graph_nodes WHERE id = ?"
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;

    Ok(r.map(|r| parse_node(r.id, r.label, r.properties, r.document_id, r.database_name, r.collection_name, r.created_at)))
}

pub async fn find_nodes_by_label(
    pool: &SqlitePool,
    label: &str,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<GraphNode>> {
    #[derive(sqlx::FromRow)]
    struct Row { id: String, label: String, properties: String, document_id: Option<String>, database_name: String, collection_name: String, created_at: String }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, label, properties, document_id, database_name, collection_name, created_at FROM graph_nodes WHERE label = ? AND database_name = ? AND collection_name = ?"
    )
    .bind(label)
    .bind(database_name)
    .bind(collection_name)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| parse_node(r.id, r.label, r.properties, r.document_id, r.database_name, r.collection_name, r.created_at)).collect())
}

pub async fn delete_node(pool: &SqlitePool, node_id: &str) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM graph_nodes WHERE id = ?")
        .bind(node_id)
        .execute(pool)
        .await?
        .rows_affected() > 0)
}

pub async fn delete_edge(pool: &SqlitePool, edge_id: &str) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM graph_edges WHERE id = ?")
        .bind(edge_id)
        .execute(pool)
        .await?
        .rows_affected() > 0)
}

pub async fn get_neighbors(
    pool: &SqlitePool,
    node_id: &str,
    relation: Option<&str>,
    direction: &str,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<(GraphNode, GraphEdge)>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        nid: String, label: String, properties: String, document_id: Option<String>,
        ndatabase: String, ncollection: String, created_at: String,
        eid: String, source_id: String, target_id: String, relation: String, weight: f64, eprops: String, ecreated: String,
    }

    let rows: Vec<Row> = match (direction, relation) {
        ("out", Some(rel)) => sqlx::query_as(
            "SELECT n.id as nid, n.label, n.properties, n.document_id, n.database_name as ndatabase, n.collection_name as ncollection, n.created_at,
                    e.id as eid, e.source_id, e.target_id, e.relation, e.weight, e.properties as eprops, e.created_at as ecreated
             FROM graph_edges e JOIN graph_nodes n ON e.target_id = n.id
             WHERE e.source_id = ? AND e.relation = ? AND n.database_name = ? AND n.collection_name = ?"
        ).bind(node_id).bind(rel).bind(database_name).bind(collection_name).fetch_all(pool).await?,

        ("out", None) => sqlx::query_as(
            "SELECT n.id as nid, n.label, n.properties, n.document_id, n.database_name as ndatabase, n.collection_name as ncollection, n.created_at,
                    e.id as eid, e.source_id, e.target_id, e.relation, e.weight, e.properties as eprops, e.created_at as ecreated
             FROM graph_edges e JOIN graph_nodes n ON e.target_id = n.id
             WHERE e.source_id = ? AND n.database_name = ? AND n.collection_name = ?"
        ).bind(node_id).bind(database_name).bind(collection_name).fetch_all(pool).await?,

        ("in", Some(rel)) => sqlx::query_as(
            "SELECT n.id as nid, n.label, n.properties, n.document_id, n.database_name as ndatabase, n.collection_name as ncollection, n.created_at,
                    e.id as eid, e.source_id, e.target_id, e.relation, e.weight, e.properties as eprops, e.created_at as ecreated
             FROM graph_edges e JOIN graph_nodes n ON e.source_id = n.id
             WHERE e.target_id = ? AND e.relation = ? AND n.database_name = ? AND n.collection_name = ?"
        ).bind(node_id).bind(rel).bind(database_name).bind(collection_name).fetch_all(pool).await?,

        _ => sqlx::query_as(
            "SELECT n.id as nid, n.label, n.properties, n.document_id, n.database_name as ndatabase, n.collection_name as ncollection, n.created_at,
                    e.id as eid, e.source_id, e.target_id, e.relation, e.weight, e.properties as eprops, e.created_at as ecreated
             FROM graph_edges e JOIN graph_nodes n ON (e.target_id = n.id AND e.source_id = ?)
                                                   OR (e.source_id = n.id AND e.target_id = ?)
             WHERE n.database_name = ? AND n.collection_name = ?"
        ).bind(node_id).bind(node_id).bind(database_name).bind(collection_name).fetch_all(pool).await?,
    };

    Ok(rows
        .into_iter()
        .map(|r| (
            parse_node(r.nid, r.label, r.properties, r.document_id, r.ndatabase, r.ncollection, r.created_at),
            parse_edge(r.eid, r.source_id, r.target_id, r.relation, r.weight, r.eprops, r.ecreated),
        ))
        .collect())
}

pub async fn bfs_traverse(
    pool: &SqlitePool,
    start_node_id: &str,
    max_depth: usize,
    relation: Option<&str>,
    direction: &str,
    database_name: &str,
    collection_name: &str,
) -> Result<GraphSearchResult> {
    let mut visited_nodes: HashMap<String, GraphNode> = HashMap::new();
    let mut visited_edges: HashMap<String, GraphEdge> = HashMap::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();

    if let Some(start) = get_node(pool, start_node_id).await? {
        seen.insert(start.id.clone());
        queue.push_back((start.id.clone(), 0));
        visited_nodes.insert(start.id.clone(), start);
    } else {
        return Ok(GraphSearchResult { nodes: vec![], edges: vec![], documents: vec![] });
    }

    while let Some((node_id, depth)) = queue.pop_front() {
        if depth >= max_depth { continue; }
        for (neighbor, edge) in get_neighbors(pool, &node_id, relation, direction, database_name, collection_name).await? {
            visited_edges.entry(edge.id.clone()).or_insert(edge);
            if !seen.contains(&neighbor.id) {
                seen.insert(neighbor.id.clone());
                queue.push_back((neighbor.id.clone(), depth + 1));
                visited_nodes.insert(neighbor.id.clone(), neighbor);
            }
        }
    }

    let doc_ids: Vec<String> = visited_nodes.values()
        .filter_map(|n| n.document_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut documents = Vec::new();
    for doc_id in doc_ids {
        if let Some(doc) = crate::embedding::fetch_document(pool, &doc_id).await? {
            documents.push(doc);
        }
    }

    Ok(GraphSearchResult {
        nodes: visited_nodes.into_values().collect(),
        edges: visited_edges.into_values().collect(),
        documents,
    })
}

pub async fn list_nodes(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
    database_name: &str,
    collection_name: &str,
) -> Result<Vec<GraphNode>> {
    #[derive(sqlx::FromRow)]
    struct Row { id: String, label: String, properties: String, document_id: Option<String>, database_name: String, collection_name: String, created_at: String }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, label, properties, document_id, database_name, collection_name, created_at FROM graph_nodes WHERE database_name = ? AND collection_name = ? ORDER BY created_at DESC LIMIT ? OFFSET ?"
    )
    .bind(database_name)
    .bind(collection_name)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| parse_node(r.id, r.label, r.properties, r.document_id, r.database_name, r.collection_name, r.created_at)).collect())
}

pub async fn list_edges(pool: &SqlitePool, limit: i64, offset: i64) -> Result<Vec<GraphEdge>> {
    #[derive(sqlx::FromRow)]
    struct Row { id: String, source_id: String, target_id: String, relation: String, weight: f64, properties: String, created_at: String }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, source_id, target_id, relation, weight, properties, created_at FROM graph_edges ORDER BY created_at DESC LIMIT ? OFFSET ?"
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| parse_edge(r.id, r.source_id, r.target_id, r.relation, r.weight, r.properties, r.created_at)).collect())
}
