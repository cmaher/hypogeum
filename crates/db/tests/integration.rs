#[test]
fn integration_suite() {
    test_support::run_suite();
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
            .env("MIGRATION_DATABASE_URL", &db.migration_url)
            .env("FOUNDATION_LOCAL_CONFIG", "missing-test-config")
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
            assert!(result["data"]["tables"].as_array().unwrap().len() >= 4);
        }
    }
}

#[test]
fn cli_rejects_unknown_input_and_redacts_connection_errors() {
    for args in [vec!["bad"], vec!["migrate"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_foundation-db"))
            .args(args)
            .env(
                "MIGRATION_DATABASE_URL",
                "postgres://private-credential@127.0.0.1:1/missing",
            )
            .env("FOUNDATION_LOCAL_CONFIG", "missing-test-config")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["ok"], false);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-credential"));
    }
}
