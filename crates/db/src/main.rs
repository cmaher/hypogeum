use serde_json::{Value, json};
use std::process::ExitCode;

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
            json!({"commands":["migrate","verify","inspect"],"output":"json","configuration":"MIGRATION_DATABASE_URL","migrations":"Explicit only; dedicated migrator credentials required"}),
        );
    }
    if args.len() != 1 || !matches!(args[0].as_str(), "migrate" | "verify" | "inspect") {
        return Err(
            "Usage: foundation-db migrate|verify|inspect [--output json]; --describe for schema"
                .into(),
        );
    }
    let path =
        std::env::var("FOUNDATION_LOCAL_CONFIG").unwrap_or_else(|_| "env/local.secrets".into());
    if let Err(error) = dotenvy::from_path(path)
        && !error.not_found()
    {
        return Err("Cannot load local configuration".into());
    }
    let url =
        std::env::var("MIGRATION_DATABASE_URL").map_err(|_| "MIGRATION_DATABASE_URL required")?;
    match args[0].as_str() {
        "migrate" => {
            foundation_db::migrate(&url)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({"migrated":true}))
        }
        "verify" => {
            foundation_db::verify(&url)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({"verified":true}))
        }
        "inspect" => foundation_db::inspect(&url)
            .await
            .map_err(|e| e.to_string()),
        _ => unreachable!(),
    }
}
