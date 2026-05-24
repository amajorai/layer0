use anyhow::Result;
use sqlx::SqlitePool;

use crate::db::now_str;
use crate::types::{Collection, Database};

/// Windows reserved device names that must not be used as filenames.
/// These are case-insensitive on Windows and cause silent I/O redirection.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL",
    "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Maximum byte length for a database or collection name.
/// Set conservatively at 64 to ensure the derived `<name>.db` filename stays
/// well within the Windows MAX_PATH (260 chars) limit even when the databases
/// directory itself sits deep in a user profile path.
const MAX_NAME_LEN: usize = 64;

/// Public re-export of the name validation logic so callers outside this module
/// (e.g. `pool_for` in `layer0-server` and `layer0-mcp`) can validate a
/// database name before constructing any path or opening a pool.
pub fn validate_name_pub(name: &str) -> Result<()> {
    validate_name(name)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow::anyhow!("name cannot be empty"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(anyhow::anyhow!(
            "name cannot exceed {} characters",
            MAX_NAME_LEN
        ));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return Err(anyhow::anyhow!(
            "name may only contain letters, digits, underscores, hyphens, and dots"
        ));
    }
    // A name composed entirely of dots (e.g. "..", "...") is a path-traversal
    // fragment even when the individual characters are allowed.
    if name.chars().all(|c| c == '.') {
        return Err(anyhow::anyhow!(
            "name must not consist entirely of dots"
        ));
    }
    // Reject Windows reserved device names to prevent I/O redirection on Windows.
    let upper = name.to_uppercase();
    // Strip a trailing dot (Windows ignores trailing dots in filenames).
    let stem = upper.trim_end_matches('.');
    if WINDOWS_RESERVED.contains(&stem) {
        return Err(anyhow::anyhow!(
            "name '{}' is a reserved Windows device name and cannot be used",
            name
        ));
    }
    Ok(())
}

pub async fn ensure_database(pool: &SqlitePool, name: &str) -> Result<()> {
    validate_name(name)?;
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
    validate_name(database_name)?;
    validate_name(name)?;
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

pub async fn create_database(pool: &SqlitePool, config: &crate::config::Config, name: &str, description: Option<&str>) -> Result<Database> {
    validate_name(name)?;
    let now = now_str();
    sqlx::query(
        "INSERT INTO databases (name, description, created_at) VALUES (?, ?, ?)"
    )
    .bind(name)
    .bind(description)
    .bind(&now)
    .execute(pool)
    .await?;

    // Create the dedicated .db file for this database
    if name != "default" {
        let db_pool = crate::db::connect_database(config, name).await?;
        db_pool.close().await;
    }

    Ok(Database {
        name: name.to_string(),
        description: description.map(String::from),
        created_at: crate::db::parse_dt(&now),
    })
}

pub async fn delete_database(pool: &SqlitePool, config: &crate::config::Config, name: &str) -> Result<bool> {
    // Validate first — must happen before any FS or DB operation.
    validate_name(name)?;
    if name == "default" {
        return Err(anyhow::anyhow!("cannot delete the default database"));
    }
    // Remove collections from registry
    sqlx::query("DELETE FROM collections WHERE database_name = ?").bind(name).execute(pool).await?;
    // Remove from databases registry
    let r = sqlx::query("DELETE FROM databases WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await?;
    // Build the candidate path and verify it is inside databases_dir before deleting.
    let databases_dir = config.databases_dir().canonicalize()
        .unwrap_or_else(|_| config.databases_dir());
    let db_path = config.databases_dir().join(format!("{}.db", name));
    if db_path.exists() {
        // Canonicalize to resolve any remaining symlinks / relative components.
        let canonical = db_path.canonicalize()?;
        if !canonical.starts_with(&databases_dir) {
            return Err(anyhow::anyhow!(
                "resolved path '{}' is outside the databases directory — refusing to delete",
                canonical.display()
            ));
        }
        std::fs::remove_file(&canonical)?;
    }
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
    validate_name(database_name)?;
    validate_name(name)?;
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
    crate::embedding::purge_collection_vectors(pool, database_name, Some(name)).await?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM documents WHERE database_name = ? AND collection_name = ?"
    )
    .bind(database_name)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM graph_nodes WHERE database_name = ? AND collection_name = ?"
    )
    .bind(database_name)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    let r = sqlx::query(
        "DELETE FROM collections WHERE database_name = ? AND name = ?"
    )
    .bind(database_name)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_name unit tests (pure, no async) ──────────────────────────────

    fn ok(name: &str) {
        validate_name(name).unwrap_or_else(|e| panic!("expected Ok for {:?}: {}", name, e));
    }

    fn err(name: &str) {
        validate_name(name).expect_err(&format!("expected Err for {:?}", name));
    }

    // Category 1: Boundary values
    #[test]
    fn boundary_empty_name_is_rejected() {
        err("");
    }

    #[test]
    fn boundary_exactly_max_len_chars_is_accepted() {
        let name = "a".repeat(MAX_NAME_LEN);
        ok(&name);
    }

    #[test]
    fn boundary_one_over_max_len_is_rejected() {
        let name = "a".repeat(MAX_NAME_LEN + 1);
        err(&name);
    }

    #[test]
    fn boundary_256_chars_is_rejected() {
        // 256 > MAX_NAME_LEN; kept as explicit regression guard.
        let name = "a".repeat(256);
        err(&name);
    }

    #[test]
    fn boundary_default_name_is_accepted() {
        // "default" is a valid name per validate_name; business logic elsewhere
        // protects it from deletion.
        ok("default");
    }

    // Category 2: Valid normal names
    #[test]
    fn valid_alphanumeric_underscore_hyphen_dot() {
        ok("my-db_01.v2");
    }

    // Category 3: Invalid input formats
    #[test]
    fn invalid_slash_is_rejected() {
        err("foo/bar");
    }

    #[test]
    fn invalid_backslash_is_rejected() {
        err("foo\\bar");
    }

    #[test]
    fn invalid_space_is_rejected() {
        err("foo bar");
    }

    #[test]
    fn invalid_unicode_is_rejected() {
        // Non-ASCII letters are rejected because is_alphanumeric covers unicode,
        // so this tests that unicode *letters* actually pass but non-letter unicode
        // like emoji is rejected.
        err("foo\u{1F600}bar");
    }

    #[test]
    fn invalid_sql_injection_is_rejected() {
        err("'; DROP TABLE databases; --");
    }

    // P0: null byte must be rejected
    #[test]
    fn p0_null_byte_is_rejected() {
        err("foo\0bar");
        err("\0");
        err("abc\0");
    }

    // P0: path traversal — dotdot without slash is caught by the all-dots rule
    #[test]
    fn p0_dotdot_only_is_rejected() {
        err("..");
        err("...");
    }

    #[test]
    fn single_dot_only_is_rejected() {
        err(".");
    }

    // The slash in "../etc" is already blocked by the charset rule, so this is
    // belt-and-suspenders; the important case is pure ".." above.
    #[test]
    fn p0_path_traversal_with_slash_is_rejected() {
        err("../etc");
        err("../../secret");
    }

    // P1: Windows reserved names
    #[test]
    fn p1_windows_reserved_nul_is_rejected() {
        err("NUL");
        err("nul");
        err("Nul");
    }

    #[test]
    fn p1_windows_reserved_con_is_rejected() {
        err("CON");
        err("con");
    }

    #[test]
    fn p1_windows_reserved_prn_aux_are_rejected() {
        err("PRN");
        err("AUX");
    }

    #[test]
    fn p1_windows_reserved_com_lpt_are_rejected() {
        for i in 0..=9 {
            err(&format!("COM{}", i));
            err(&format!("LPT{}", i));
            err(&format!("com{}", i));
            err(&format!("lpt{}", i));
        }
    }

    // Names that merely start with a reserved prefix but are longer are fine.
    #[test]
    fn names_starting_with_reserved_prefix_are_allowed() {
        ok("null");        // "null" != "NUL"
        ok("console");     // starts with "con" but not exactly "CON"
        ok("NULLdb");
    }

    // ── Async integration tests using a real in-memory SQLite pool ─────────────

    async fn make_pool() -> SqlitePool {
        use sqlx::sqlite::SqlitePoolOptions;
        // sqlite-vec registration required before connecting.
        unsafe {
            static INIT: std::sync::Once = std::sync::Once::new();
            INIT.call_once(|| {
                libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute(
                    sqlite_vec::sqlite3_vec_init as *const (),
                )));
            });
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        crate::db::run_migrations(&pool, 4).await.expect("migrations");
        pool
    }

    fn make_config(tmp: &std::path::Path) -> crate::config::Config {
        let mut cfg = crate::config::Config::default();
        cfg.database.path = tmp.join("layer0.db");
        cfg
    }

    // P0: double create_database must fail (not silently ignore).
    #[tokio::test]
    async fn p0_double_create_database_fails() {
        let pool = make_pool().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        create_database(&pool, &cfg, "mydb", None).await.expect("first create ok");
        let second = create_database(&pool, &cfg, "mydb", None).await;
        assert!(second.is_err(), "second create_database for same name must fail");
    }

    // P1: deleting a non-existent database returns Ok(false), does not panic.
    #[tokio::test]
    async fn p1_delete_nonexistent_database_returns_false() {
        let pool = make_pool().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        let result = delete_database(&pool, &cfg, "doesnotexist").await;
        assert!(result.is_ok(), "delete non-existent should be Ok");
        assert_eq!(result.unwrap(), false, "should report 0 rows affected");
    }

    // State machine: deleting "default" must be rejected.
    #[tokio::test]
    async fn state_delete_default_database_is_rejected() {
        let pool = make_pool().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        let result = delete_database(&pool, &cfg, "default").await;
        assert!(result.is_err(), "deleting 'default' must return an error");
    }

    // State machine: listing collections of a deleted (or never-created) database
    // should return an empty list, not an error.
    #[tokio::test]
    async fn state_list_collections_of_nonexistent_database_is_empty() {
        let pool = make_pool().await;
        let cols = list_collections(&pool, "ghost_db").await.expect("list_collections should not error");
        assert!(cols.is_empty(), "non-existent database should have no collections");
    }

    // Null/missing: description=None is stored and round-tripped correctly.
    #[tokio::test]
    async fn null_description_is_stored_as_none() {
        let pool = make_pool().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        let db = create_database(&pool, &cfg, "nodesc", None).await.expect("create ok");
        assert_eq!(db.description, None);

        let fetched = get_database(&pool, "nodesc").await.expect("get ok").expect("exists");
        assert_eq!(fetched.description, None);
    }

    // Boundary: exactly MAX_NAME_LEN-char name is accepted and can be created.
    #[tokio::test]
    async fn boundary_max_length_name_creates_successfully() {
        let pool = make_pool().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());
        let name = "a".repeat(MAX_NAME_LEN);

        let db = create_database(&pool, &cfg, &name, None).await
            .expect("max-length name should be accepted");
        assert_eq!(db.name.len(), MAX_NAME_LEN);
    }
}
