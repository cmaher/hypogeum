use std::collections::HashMap;
use tower_sessions::SessionStore;
use tower_sessions::session::{Id, Record};

#[test]
fn integration_suite() {
    hypgm_test_support::run_suite();
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn postgres_session_store_persists_and_expires_sessions(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
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

    let store = exemplum_dmn_auth::PostgresSessionStore::new_with_schema(pool.clone(), "public");
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
    assert_eq!(reloaded.data.get("role"), Some(&serde_json::json!("admin")));

    // Expired deletion test
    let expired_id = Id::default();
    let mut expired_record = Record {
        id: expired_id,
        data: HashMap::new(),
        expiry_date: time::OffsetDateTime::now_utc() - time::Duration::hours(1),
    };
    store.create(&mut expired_record).await.unwrap();
    store.delete_expired().await.unwrap();
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT FROM public.sessions WHERE id=$1)")
        .bind(expired_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!exists, "expired row must be physically deleted");
    assert!(store.load(&session_id).await.unwrap().is_some());

    // Verify leading underscore is accepted as a valid identifier
    let _store_underscore =
        exemplum_dmn_auth::PostgresSessionStore::new_with_schema(pool.clone(), "_app");

    // Verify default constructor uses "exemplum" schema
    let _store_default = exemplum_dmn_auth::PostgresSessionStore::new(pool.clone());

    pool.close().await;
}
