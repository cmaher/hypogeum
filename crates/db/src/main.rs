use env_config::EnvSource;
use serde::Deserialize;
use serde_json::{Value, json};
use std::process::ExitCode;

#[derive(Deserialize)]
struct Config {
    database_url: env_config::Secret,
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
            json!({"commands":["migrate","verify","inspect"],"output":"json","configuration":"APP_MIGRATION_DATABASE_URL","migrations":"Explicit only; dedicated migrator credentials required"}),
        );
    }
    if args.len() != 1 || !matches!(args[0].as_str(), "migrate" | "verify" | "inspect") {
        return Err(
            "Usage: foundation-db migrate|verify|inspect [--output json]; --describe for schema"
                .into(),
        );
    }

    let env = EnvSource::load().map_err(|e| e.to_string())?;
    let config: Config = env
        .deserialize("APP_MIGRATION_")
        .map_err(|e| e.to_string())?;
    let url = config.database_url.expose();

    match args[0].as_str() {
        "migrate" => {
            foundation_db::migrate(url)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({"migrated":true}))
        }
        "verify" => {
            foundation_db::verify(url)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({"verified":true}))
        }
        "inspect" => foundation_db::inspect(url)
            .await
            .map_err(|e| e.to_string()),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use env_config::EnvSource;

    #[test]
    fn test_config_loads_with_url() {
        let env = EnvSource::from_pairs([(
            "APP_MIGRATION_DATABASE_URL",
            "postgres://user:pass@localhost/db",
        )]);
        let config: Config = env.deserialize("APP_MIGRATION_").unwrap();
        assert_eq!(
            config.database_url.expose(),
            "postgres://user:pass@localhost/db"
        );
    }

    #[test]
    fn test_config_missing_url_error_names_app_migration_database_url() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let err = env.deserialize::<Config>("APP_MIGRATION_").unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("APP_MIGRATION_DATABASE_URL"));
    }
}
