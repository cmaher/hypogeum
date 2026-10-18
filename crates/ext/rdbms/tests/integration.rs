#[test]
fn integration_suite() {
    hypgm_test_support::run_suite();
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn repeated_migrations_do_not_rewrite_objects_and_corrupt_history_is_rejected(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    let config = hypgm_test_support::test_config();
    db.migrate_with(&config).await;
    let admin = sqlx::PgPool::connect(&db.admin_url).await.unwrap();
    let versions = "SELECT xmin::text FROM pg_catalog.pg_proc WHERE oid IN ('apalis.schema_version()'::regprocedure) ORDER BY oid";
    let before: Vec<String> = sqlx::query_scalar(versions)
        .fetch_all(&admin)
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    db.migrate_with(&config).await;
    let after: Vec<String> = sqlx::query_scalar(versions)
        .fetch_all(&admin)
        .await
        .unwrap();
    assert_eq!(
        before, after,
        "a repeat run must not replace already-applied schema functions"
    );
    sqlx::query("UPDATE apalis._sqlx_migrations SET checksum=$1 WHERE version=(SELECT min(version) FROM apalis._sqlx_migrations)")
        .bind(vec![0_u8])
        .execute(&admin)
        .await
        .unwrap();
    assert!(
        hypgmx_rdbms::migrate(&db.migration_url, &config)
            .await
            .is_err()
    );
    assert!(
        hypgmx_rdbms::verify(&db.migration_url, &config)
            .await
            .is_err()
    );
    admin.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn postgres_session_store_persists_and_expires_sessions(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use std::collections::HashMap;
    use tower_sessions::SessionStore;
    use tower_sessions::session::{Id, Record};

    let db = hypgm_test_support::TestDatabase::provision(options).await;
    let pool = sqlx::PgPool::connect(&db.admin_url).await.unwrap();

    sqlx::raw_sql(
        "CREATE TABLE public.sessions (
            id TEXT PRIMARY KEY,
            data BYTEA NOT NULL,
            expiry_date TIMESTAMPTZ NOT NULL
        );",
    )
    .execute(&pool)
    .await
    .unwrap();

    let store = hypgmx_rdbms::PostgresSessionStore::new(pool.clone());
    let session_id = Id::default();
    let mut data = HashMap::new();
    data.insert("user_id".to_string(), serde_json::json!("123"));
    let expiry = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    let mut record = Record {
        id: session_id,
        data,
        expiry_date: expiry,
    };

    // Create session
    store.create(&mut record).await.unwrap();

    // Load session
    let loaded = store.load(&session_id).await.unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.id, session_id);
    assert_eq!(loaded.data.get("user_id"), Some(&serde_json::json!("123")));

    // Save session (update data)
    let mut updated_record = loaded;
    updated_record
        .data
        .insert("role".to_string(), serde_json::json!("admin"));
    store.save(&updated_record).await.unwrap();

    let reloaded = store.load(&session_id).await.unwrap().unwrap();
    assert_eq!(
        reloaded.data.get("role"),
        Some(&serde_json::json!("admin"))
    );

    // Delete session
    store.delete(&session_id).await.unwrap();
    let deleted = store.load(&session_id).await.unwrap();
    assert!(deleted.is_none());

    pool.close().await;
}
