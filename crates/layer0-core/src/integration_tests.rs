/// End-to-end integration tests that exercise the real code paths from database
/// creation through document storage and retrieval, using real SQLite files on disk.
///
/// Each test gets its own `tempfile::TempDir` so they are fully isolated.
#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::database::{
        create_collection, create_database, delete_collection, delete_database,
        ensure_collection, get_collection, list_collections, list_databases,
    };
    use crate::db::{connect, connect_database, now_str};
    use crate::embedding::fetch_document;

    // ── helpers ────────────────────────────────────────────────────────────────

    /// Register sqlite-vec exactly once across the entire test process.
    fn register_vec() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| unsafe {
            libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        });
    }

    /// Build a minimal `Config` whose database files live inside `dir`.
    /// The default database is `dir/layer0.db`; named databases go in
    /// `dir/databases/<name>.db`.
    fn make_config(dir: &std::path::Path) -> Config {
        let mut cfg = Config::default();
        cfg.database.path = dir.join("layer0.db");
        // Use a single connection to make pool teardown deterministic on Windows.
        cfg.database.max_connections = 1;
        cfg
    }

    /// Insert a bare-bones document row (no embeddings) into `pool`.
    async fn insert_document(
        pool: &sqlx::SqlitePool,
        id: &str,
        content: &str,
        database_name: &str,
        collection_name: &str,
    ) -> anyhow::Result<()> {
        let now = now_str();
        sqlx::query(
            "INSERT INTO documents
             (id, content, metadata, source, database_name, collection_name, created_at, updated_at)
             VALUES (?, ?, '{}', NULL, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(content)
        .bind(database_name)
        .bind(collection_name)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Count documents in `pool` that belong to `database_name`.
    async fn count_docs_in_db(
        pool: &sqlx::SqlitePool,
        database_name: &str,
    ) -> anyhow::Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM documents WHERE database_name = ?",
        )
        .bind(database_name)
        .fetch_one(pool)
        .await?;
        Ok(n)
    }

    /// Close a pool and ensure the WAL is checkpointed so that Windows releases
    /// the file handles before we attempt to remove the file.
    async fn close_and_checkpoint(pool: sqlx::SqlitePool) {
        // Checkpoint the WAL back into the main file so no WAL/SHM handles remain.
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await;
        let _ = sqlx::query("PRAGMA journal_mode=DELETE")
            .execute(&pool)
            .await;
        pool.close().await;
    }

    // ── Flow 1: Create → store → retrieve → delete (golden path) ──────────────
    //
    // create_database "testdb"
    // → connect_database to "testdb"
    // → ensure_collection in "testdb"/"default"
    // → insert a document into "testdb"
    // → verify the document exists in "testdb"
    // → verify the document does NOT appear when querying the default database
    // → delete_database "testdb"
    // → verify the .db file no longer exists
    #[tokio::test]
    async fn flow1_create_store_retrieve_delete() {
        register_vec();
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        // Initialise the default (registry) database.
        let registry = connect(&cfg).await.expect("connect default");

        // Create the named database and connect to its dedicated file.
        create_database(&registry, &cfg, "testdb", None)
            .await
            .expect("create_database testdb");
        let testdb_pool = connect_database(&cfg, "testdb")
            .await
            .expect("connect testdb");

        // Ensure the collection exists in the registry.
        ensure_collection(&registry, "testdb", "default")
            .await
            .expect("ensure_collection");

        // Insert a document into the testdb file.
        let doc_id = "doc-flow1-001";
        insert_document(&testdb_pool, doc_id, "hello from testdb", "testdb", "default")
            .await
            .expect("insert_document");

        // Document must be visible when fetched from testdb.
        let found = fetch_document(&testdb_pool, doc_id)
            .await
            .expect("fetch from testdb")
            .expect("doc must exist");
        assert_eq!(found.content, "hello from testdb");
        assert_eq!(found.database_name, "testdb");

        // Document must NOT appear in the default database pool.
        let not_found = fetch_document(&registry, doc_id)
            .await
            .expect("fetch from default");
        assert!(
            not_found.is_none(),
            "document stored in testdb must not appear in the default database"
        );

        // Record the expected .db file path before deletion.
        let db_file = cfg.databases_dir().join("testdb.db");
        assert!(db_file.exists(), "testdb.db should exist before deletion");

        // Close the named-db pool fully (checkpoint WAL) before removing the file.
        close_and_checkpoint(testdb_pool).await;

        // Delete database via the registry.
        let deleted = delete_database(&registry, &cfg, "testdb")
            .await
            .expect("delete_database");
        assert!(deleted, "delete_database should return true");

        // The .db file must be gone.
        assert!(
            !db_file.exists(),
            "testdb.db file should be removed after delete_database"
        );

        close_and_checkpoint(registry).await;
    }

    // ── Flow 2: Collections within a named database ────────────────────────────
    //
    // create_database "mydb"
    // → create_collection "mydb"/"col1"
    // → create_collection "mydb"/"col2"
    // → list_collections "mydb" → returns both
    // → delete_collection "mydb"/"col1"
    // → list_collections "mydb" → returns only col2
    // → delete_database "mydb"
    #[tokio::test]
    async fn flow2_collections_within_named_database() {
        register_vec();
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        let registry = connect(&cfg).await.expect("connect default");

        create_database(&registry, &cfg, "mydb", None)
            .await
            .expect("create mydb");

        create_collection(&registry, "mydb", "col1", None)
            .await
            .expect("create col1");
        create_collection(&registry, "mydb", "col2", None)
            .await
            .expect("create col2");

        // list_collections should return both.
        let cols = list_collections(&registry, "mydb")
            .await
            .expect("list_collections");
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"col1"), "col1 should be listed");
        assert!(names.contains(&"col2"), "col2 should be listed");
        assert_eq!(cols.len(), 2, "exactly two collections expected");

        // Delete col1.
        let deleted = delete_collection(&registry, "mydb", "col1")
            .await
            .expect("delete_collection col1");
        assert!(deleted, "delete_collection should return true");

        // list_collections should now return only col2.
        let cols_after = list_collections(&registry, "mydb")
            .await
            .expect("list_collections after delete");
        let names_after: Vec<&str> = cols_after.iter().map(|c| c.name.as_str()).collect();
        assert!(
            !names_after.contains(&"col1"),
            "col1 must not appear after deletion"
        );
        assert!(names_after.contains(&"col2"), "col2 must still be listed");
        assert_eq!(cols_after.len(), 1, "exactly one collection expected");

        // Verify col1 is gone, col2 still reachable via get_collection.
        let gone = get_collection(&registry, "mydb", "col1")
            .await
            .expect("get_collection col1");
        assert!(gone.is_none(), "col1 must not exist after deletion");

        let still = get_collection(&registry, "mydb", "col2")
            .await
            .expect("get_collection col2");
        assert!(still.is_some(), "col2 must still exist");

        // Cleanup: checkpoint and close the mydb file before deletion.
        let mydb_pool = connect_database(&cfg, "mydb")
            .await
            .expect("connect mydb for cleanup");
        close_and_checkpoint(mydb_pool).await;

        delete_database(&registry, &cfg, "mydb")
            .await
            .expect("delete mydb");

        close_and_checkpoint(registry).await;
    }

    // ── Flow 3: Default database isolation ────────────────────────────────────
    //
    // store document in default database
    // → create named database "otherdb"
    // → verify document is NOT in "otherdb" (file isolation)
    // → store document in "otherdb"
    // → verify otherdb document is NOT in default
    // → delete "otherdb"
    // → verify default document still exists
    #[tokio::test]
    async fn flow3_default_database_isolation() {
        register_vec();
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        let default_pool = connect(&cfg).await.expect("connect default");

        // Insert a document into the default database.
        let default_doc_id = "doc-flow3-default";
        insert_document(
            &default_pool,
            default_doc_id,
            "default db document",
            "default",
            "default",
        )
        .await
        .expect("insert into default");

        // Create "otherdb" and connect to it.
        create_database(&default_pool, &cfg, "otherdb", None)
            .await
            .expect("create otherdb");
        let other_pool = connect_database(&cfg, "otherdb")
            .await
            .expect("connect otherdb");

        // The default document must NOT appear in otherdb.
        let not_in_other = fetch_document(&other_pool, default_doc_id)
            .await
            .expect("fetch from otherdb");
        assert!(
            not_in_other.is_none(),
            "document stored in default must not appear in otherdb"
        );

        // Insert a document into otherdb.
        let other_doc_id = "doc-flow3-other";
        insert_document(&other_pool, other_doc_id, "otherdb document", "otherdb", "default")
            .await
            .expect("insert into otherdb");

        // The otherdb document must NOT appear in the default database.
        let not_in_default = fetch_document(&default_pool, other_doc_id)
            .await
            .expect("fetch from default");
        assert!(
            not_in_default.is_none(),
            "document stored in otherdb must not appear in default"
        );

        // Cross-check document counts for clarity.
        let default_count = count_docs_in_db(&default_pool, "default")
            .await
            .expect("count default docs");
        assert_eq!(default_count, 1, "default db should have exactly 1 document");

        let other_count = count_docs_in_db(&other_pool, "otherdb")
            .await
            .expect("count otherdb docs");
        assert_eq!(other_count, 1, "otherdb should have exactly 1 document");

        // Close otherdb pool (with WAL checkpoint) before deleting.
        close_and_checkpoint(other_pool).await;

        delete_database(&default_pool, &cfg, "otherdb")
            .await
            .expect("delete otherdb");

        // Default document must still exist.
        let still_there = fetch_document(&default_pool, default_doc_id)
            .await
            .expect("fetch default doc after otherdb deletion");
        assert!(
            still_there.is_some(),
            "default document must survive deletion of otherdb"
        );
        assert_eq!(
            still_there.unwrap().content,
            "default db document",
            "default document content must be intact"
        );

        close_and_checkpoint(default_pool).await;
    }

    // ── Flow 4: Registry consistency ──────────────────────────────────────────
    //
    // create_database "db1"
    // create_database "db2"
    // list_databases → returns default, db1, db2
    // delete_database "db1"
    // list_databases → returns default, db2
    // → verify db1.db file is gone
    // → verify db2.db file still exists
    // → cleanup: delete_database "db2"
    #[tokio::test]
    async fn flow4_registry_consistency() {
        register_vec();
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        let registry = connect(&cfg).await.expect("connect default");

        create_database(&registry, &cfg, "db1", None)
            .await
            .expect("create db1");
        create_database(&registry, &cfg, "db2", None)
            .await
            .expect("create db2");

        // list_databases must return default, db1, and db2.
        let dbs = list_databases(&registry).await.expect("list_databases");
        let db_names: Vec<&str> = dbs.iter().map(|d| d.name.as_str()).collect();
        assert!(db_names.contains(&"default"), "default must be listed");
        assert!(db_names.contains(&"db1"), "db1 must be listed");
        assert!(db_names.contains(&"db2"), "db2 must be listed");
        assert_eq!(dbs.len(), 3, "exactly three databases expected");

        let db1_file = cfg.databases_dir().join("db1.db");
        let db2_file = cfg.databases_dir().join("db2.db");
        assert!(db1_file.exists(), "db1.db must exist");
        assert!(db2_file.exists(), "db2.db must exist");

        // Checkpoint db1 before deletion.
        let db1_pool = connect_database(&cfg, "db1")
            .await
            .expect("connect db1 for checkpoint");
        close_and_checkpoint(db1_pool).await;

        // Delete db1.
        delete_database(&registry, &cfg, "db1")
            .await
            .expect("delete db1");

        // list_databases must now return only default and db2.
        let dbs_after = list_databases(&registry).await.expect("list_databases after delete");
        let names_after: Vec<&str> = dbs_after.iter().map(|d| d.name.as_str()).collect();
        assert!(names_after.contains(&"default"), "default must still be listed");
        assert!(
            !names_after.contains(&"db1"),
            "db1 must not be listed after deletion"
        );
        assert!(names_after.contains(&"db2"), "db2 must still be listed");
        assert_eq!(dbs_after.len(), 2, "exactly two databases expected after deletion");

        // File-level verification.
        assert!(!db1_file.exists(), "db1.db must be removed");
        assert!(db2_file.exists(), "db2.db must still exist");

        // Checkpoint db2 before cleanup deletion.
        let db2_pool = connect_database(&cfg, "db2")
            .await
            .expect("connect db2 for checkpoint");
        close_and_checkpoint(db2_pool).await;

        // Cleanup.
        delete_database(&registry, &cfg, "db2")
            .await
            .expect("delete db2");
        assert!(!db2_file.exists(), "db2.db must be removed after cleanup");

        close_and_checkpoint(registry).await;
    }

    // ── Bonus: ensure_dirs creates the databases sub-directory ────────────────
    #[tokio::test]
    async fn bonus_ensure_dirs_creates_databases_subdir() {
        register_vec();
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        cfg.ensure_dirs().expect("ensure_dirs");

        let databases_dir = cfg.databases_dir();
        assert!(
            databases_dir.exists(),
            "databases sub-directory must be created by ensure_dirs"
        );
    }

    // ── Bonus: create_database rejects duplicate names ────────────────────────
    #[tokio::test]
    async fn bonus_create_duplicate_database_fails() {
        register_vec();
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = make_config(tmp.path());

        let registry = connect(&cfg).await.expect("connect");

        create_database(&registry, &cfg, "dupdb", None)
            .await
            .expect("first create ok");

        let second = create_database(&registry, &cfg, "dupdb", None).await;
        assert!(
            second.is_err(),
            "creating a database with a duplicate name must fail"
        );

        // Checkpoint dupdb before the temp dir is dropped.
        let dupdb_pool = connect_database(&cfg, "dupdb")
            .await
            .expect("connect dupdb for cleanup");
        close_and_checkpoint(dupdb_pool).await;

        close_and_checkpoint(registry).await;
    }
}
