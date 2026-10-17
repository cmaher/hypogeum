#[test]
fn integration_suite() {
    hypgm_test_support::run_suite();
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn explicit_cli_migrates_verifies_and_inspects(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    for command in ["migrate", "migrate", "verify", "inspect"] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_exemplum-db"))
            .args([command, "--output", "json"])
            .env(
                "EXEMPLUM_MIGRATION_DATABASE_URL",
                &db.exemplum_migration_url,
            )
            .env("EXEMPLUM_ENV_FILE", "")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "command {} failed: stderr={}",
            command,
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["ok"], true);
        if command == "inspect" {
            let tables = result["data"]["tables"].as_array().unwrap();
            assert!(tables.len() >= 4);
            assert!(tables.iter().any(|t| t == "exemplum.users"));
            assert!(tables.iter().any(|t| t == "exemplum.seaql_migrations"));
            assert!(tables.iter().any(|t| t == "apalis._sqlx_migrations"));
        }
    }
}

#[test]
fn cli_rejects_unknown_input_and_redacts_connection_errors() {
    for args in [vec!["bad"], vec!["migrate"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_exemplum-db"))
            .args(args)
            .env(
                "EXEMPLUM_MIGRATION_DATABASE_URL",
                "postgres://private-credential@127.0.0.1:1/missing",
            )
            .env("EXEMPLUM_ENV_FILE", "")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["ok"], false);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-credential"));
    }
}
