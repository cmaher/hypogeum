use hypgm_envconfig::EnvSource;
use serde::Deserialize;
use serde_json::{Value, json};
use std::process::ExitCode;

#[derive(Debug, Deserialize)]
struct Config {
    database_url: hypgm_envconfig::Secret,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(data) => {
            println!("{}", json!({"ok":true,"data":data}));
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{}", json!({"ok":false,"error":{"message":message}}));
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<Value, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.ends_with(&["--output".into(), "json".into()]) {
        args.truncate(args.len() - 2);
    }
    if args == ["--describe"] || args == ["--help"] || args == ["-h"] {
        return Ok(
            json!({"commands":["migrate","verify","inspect"],"output":"json","configuration":"EXEMPLUM_MIGRATION_DATABASE_URL","migrations":"Explicit only; dedicated migrator credentials required"}),
        );
    }
    if args.len() != 1 || !matches!(args[0].as_str(), "migrate" | "verify" | "inspect") {
        return Err(
            "Usage: exemplum-db migrate|verify|inspect [--output json]; --describe for schema"
                .into(),
        );
    }

    let env = EnvSource::load_for("EXEMPLUM").map_err(|e| e.to_string())?;
    let config: Config = env
        .deserialize("EXEMPLUM_MIGRATION_")
        .map_err(|e| e.to_string())?;
    let url = config.database_url.expose();

    let db_config =
        hypgmx_rdbms::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
            .with_app_schema("exemplum")
            .with_domain_migrator(<dyn hypgmx_rdbms::DomainMigrator>::from_sea_orm::<
                exemplum_dmn_users::pg::migrations::Migrator,
            >())
            .with_service_contract(hypgmx_rdbms::ServiceContract::new(
                "exemplum",
                "exemplum_users",
                1,
                Some("m20260101_000001_create_users"),
                "users",
            ));

    match args[0].as_str() {
        "migrate" => {
            hypgmx_rdbms::migrate(url, &db_config)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({"migrated":true}))
        }
        "verify" => {
            hypgmx_rdbms::verify(url, &db_config)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({"verified":true}))
        }
        "inspect" => hypgmx_rdbms::inspect(url, &db_config)
            .await
            .map_err(|e| e.to_string()),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypgm_envconfig::EnvSource;

    #[test]
    fn test_config_loads_with_url() {
        let env = EnvSource::from_pairs([(
            "EXEMPLUM_MIGRATION_DATABASE_URL",
            "postgres://user:pass@localhost/db",
        )]);
        let config: Config = env.deserialize("EXEMPLUM_MIGRATION_").unwrap();
        assert_eq!(
            config.database_url.expose(),
            "postgres://user:pass@localhost/db"
        );
    }

    #[test]
    fn test_config_missing_url_error_names_exemplum_migration_database_url() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let err = env
            .deserialize::<Config>("EXEMPLUM_MIGRATION_")
            .unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("EXEMPLUM_MIGRATION_DATABASE_URL"));
    }
}
