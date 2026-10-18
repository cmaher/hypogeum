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
async fn inspection_preserves_multiple_contracts_in_one_schema(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    let mut config = hypgm_test_support::test_config();
    db.migrate_with(&config).await;
    for table in ["jobs", "_sqlx_migrations"] {
        config
            .service_contracts
            .push(hypgmx_rdbms::ServiceContract::new(
                "apalis",
                "hypogeum_tasks",
                1,
                Option::<String>::None,
                table,
            ));
    }
    let result = hypgmx_rdbms::inspect(&db.migration_url, &config)
        .await
        .unwrap();
    let contracts = result["contracts"]["apalis"]
        .as_array()
        .expect("schema contracts must be a list");
    assert_eq!(contracts.len(), 2);
    assert_eq!(contracts[0]["table"], "jobs");
    assert_eq!(contracts[1]["table"], "_sqlx_migrations");
}
