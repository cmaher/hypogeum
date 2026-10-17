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
        hypgm_ext_rdbms::migrate(&db.migration_url, &config)
            .await
            .is_err()
    );
    assert!(
        hypgm_ext_rdbms::verify(&db.migration_url, &config)
            .await
            .is_err()
    );
    admin.close().await;
}
