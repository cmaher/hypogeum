#[test]
fn integration_suite() {
    test_support::run_suite();
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn repeated_migrations_do_not_rewrite_objects_and_corrupt_history_is_rejected(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let admin = sqlx::PgPool::connect(&db.admin_url).await.unwrap();
    let versions = "SELECT xmin::text FROM pg_catalog.pg_proc WHERE oid IN ('fndapp.schema_version()'::regprocedure,'apalis.schema_version()'::regprocedure) ORDER BY oid";
    let before: Vec<String> = sqlx::query_scalar(versions)
        .fetch_all(&admin)
        .await
        .unwrap();
    assert_eq!(before.len(), 2);
    db.migrate().await;
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
    assert!(foundation_db::migrate(&db.migration_url).await.is_err());
    assert!(foundation_db::verify(&db.migration_url).await.is_err());
    admin.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn explicit_cli_migrates_verifies_and_inspects(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = test_support::TestDatabase::provision(options).await;
    for command in ["migrate", "migrate", "verify", "inspect"] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_foundation-db"))
            .args([command, "--output", "json"])
            .env("APP_MIGRATION_DATABASE_URL", &db.migration_url)
            .env("APP_ENV_FILE", "")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["ok"], true);
        if command == "inspect" {
            let tables = result["data"]["tables"].as_array().unwrap();
            assert!(tables.len() >= 4);
            assert!(tables.iter().any(|t| t["schema"] == "fndapp" && t["table"] == "users"));
            assert!(tables.iter().any(|t| t["schema"] == "fndapp" && t["table"] == "seaql_migrations"));
            assert!(tables.iter().any(|t| t["schema"] == "apalis" && t["table"] == "_sqlx_migrations"));
        }
    }
}

#[test]
fn cli_rejects_unknown_input_and_redacts_connection_errors() {
    for args in [vec!["bad"], vec!["migrate"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_foundation-db"))
            .args(args)
            .env(
                "APP_MIGRATION_DATABASE_URL",
                "postgres://private-credential@127.0.0.1:1/missing",
            )
            .env("APP_ENV_FILE", "")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["ok"], false);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-credential"));
    }
}
