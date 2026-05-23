use anyhow::Result;
use sqlx::SqlitePool;

use crate::db::now_str;
use crate::types::{Collection, Database};

pub async fn ensure_database(pool: &SqlitePool, name: &str) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO databases (name, created_at) VALUES (?, ?)"
    )
    .bind(name)
    .bind(now_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn ensure_collection(pool: &SqlitePool, database_name: &str, name: &str) -> Result<()> {
    ensure_database(pool, database_name).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO collections (database_name, name, created_at) VALUES (?, ?, ?)"
    )
    .bind(database_name)
    .bind(name)
    .bind(now_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_databases(pool: &SqlitePool) -> Result<Vec<Database>> {
    #[derive(sqlx::FromRow)]
    struct Row { name: String, description: Option<String>, created_at: String }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT name, description, created_at FROM databases ORDER BY created_at ASC"
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| Database {
        name: r.name,
        description: r.description,
        created_at: crate::db::parse_dt(&r.created_at),
    }).collect())
}

pub async fn get_database(pool: &SqlitePool, name: &str) -> Result<Option<Database>> {
    #[derive(sqlx::FromRow)]
    struct Row { name: String, description: Option<String>, created_at: String }

    let row: Option<Row> = sqlx::query_as(
        "SELECT name, description, created_at FROM databases WHERE name = ?"
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Database {
        name: r.name,
        description: r.description,
        created_at: crate::db::parse_dt(&r.created_at),
    }))
}

pub async fn create_database(pool: &SqlitePool, name: &str, description: Option<&str>) -> Result<Database> {
    let now = now_str();
    sqlx::query(
        "INSERT INTO databases (name, description, created_at) VALUES (?, ?, ?)"
    )
    .bind(name)
    .bind(description)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(Database {
        name: name.to_string(),
        description: description.map(String::from),
        created_at: crate::db::parse_dt(&now),
    })
}

pub async fn delete_database(pool: &SqlitePool, name: &str) -> Result<bool> {
    if name == "default" {
        return Err(anyhow::anyhow!("cannot delete the default database"));
    }
    sqlx::query("DELETE FROM documents WHERE database_name = ?").bind(name).execute(pool).await?;
    sqlx::query("DELETE FROM graph_nodes WHERE database_name = ?").bind(name).execute(pool).await?;
    sqlx::query("DELETE FROM collections WHERE database_name = ?").bind(name).execute(pool).await?;
    let r = sqlx::query("DELETE FROM databases WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn list_collections(pool: &SqlitePool, database_name: &str) -> Result<Vec<Collection>> {
    #[derive(sqlx::FromRow)]
    struct Row { database_name: String, name: String, description: Option<String>, created_at: String }

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT database_name, name, description, created_at FROM collections WHERE database_name = ? ORDER BY created_at ASC"
    )
    .bind(database_name)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| Collection {
        database_name: r.database_name,
        name: r.name,
        description: r.description,
        created_at: crate::db::parse_dt(&r.created_at),
    }).collect())
}

pub async fn get_collection(pool: &SqlitePool, database_name: &str, name: &str) -> Result<Option<Collection>> {
    #[derive(sqlx::FromRow)]
    struct Row { database_name: String, name: String, description: Option<String>, created_at: String }

    let row: Option<Row> = sqlx::query_as(
        "SELECT database_name, name, description, created_at FROM collections WHERE database_name = ? AND name = ?"
    )
    .bind(database_name)
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Collection {
        database_name: r.database_name,
        name: r.name,
        description: r.description,
        created_at: crate::db::parse_dt(&r.created_at),
    }))
}

pub async fn create_collection(
    pool: &SqlitePool,
    database_name: &str,
    name: &str,
    description: Option<&str>,
) -> Result<Collection> {
    ensure_database(pool, database_name).await?;
    let now = now_str();
    sqlx::query(
        "INSERT INTO collections (database_name, name, description, created_at) VALUES (?, ?, ?, ?)"
    )
    .bind(database_name)
    .bind(name)
    .bind(description)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(Collection {
        database_name: database_name.to_string(),
        name: name.to_string(),
        description: description.map(String::from),
        created_at: crate::db::parse_dt(&now),
    })
}

pub async fn delete_collection(pool: &SqlitePool, database_name: &str, name: &str) -> Result<bool> {
    if database_name == "default" && name == "default" {
        return Err(anyhow::anyhow!("cannot delete the default collection"));
    }
    sqlx::query(
        "DELETE FROM documents WHERE database_name = ? AND collection_name = ?"
    )
    .bind(database_name)
    .bind(name)
    .execute(pool)
    .await?;
    sqlx::query(
        "DELETE FROM graph_nodes WHERE database_name = ? AND collection_name = ?"
    )
    .bind(database_name)
    .bind(name)
    .execute(pool)
    .await?;
    let r = sqlx::query(
        "DELETE FROM collections WHERE database_name = ? AND name = ?"
    )
    .bind(database_name)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}
