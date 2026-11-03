use clap::{Parser, Subcommand, error::ErrorKind};
use exemplum_dmn_users::{
    accounts::{AccountError, AdminBootstrapService, DefaultAccountService},
    repo::PgAccountRepo,
};
use hypgm_envconfig::{EnvSource, Secret};
use hypgm_errors::RepoError;
use serde::Deserialize;
use std::{process::ExitCode, sync::Arc};

#[derive(Deserialize)]
struct Config {
    database_url: Secret,
}

/// Bootstrap Exemplum administrators. Reads EXEMPLUM_ADMIN_DATABASE_URL
/// (restricted exemplum_users role) and prints one JSON object.
#[derive(Debug, Parser)]
#[command(name = "exemplum-admin")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, PartialEq, Subcommand)]
enum Command {
    /// Approve the account that registered with USERNAME and make it an
    /// administrator. The account exists after the user's first signed-in request.
    Promote {
        #[arg(value_parser = parse_username)]
        username: String,
    },
}

fn parse_username(value: &str) -> Result<String, &'static str> {
    exemplum_dmn_users::accounts::validate_username(value)
        .map(|()| value.to_owned())
        .map_err(|_| "invalid username")
}

fn parse<I, T>(args: I) -> Result<Command, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Ok(Cli::try_parse_from(args)?.command)
}

#[tokio::main]
async fn main() -> ExitCode {
    let command = match parse(std::env::args_os()) {
        Ok(command) => command,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            let message = error.kind().as_str().unwrap_or("Invalid command arguments");
            eprintln!(
                "{}",
                serde_json::json!({"ok":false,"error":message,"usage":error.render().to_string()})
            );
            return ExitCode::FAILURE;
        }
    };
    match run(command).await {
        Ok((id, action)) => {
            println!(
                "{}",
                serde_json::json!({"ok":true,"account_id":id,"action":action})
            );
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{}", serde_json::json!({"ok":false,"error":message}));
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<(String, &'static str), String> {
    let env = EnvSource::load_for("EXEMPLUM").map_err(|_| "Could not load configuration")?;
    let config: Config = env
        .deserialize("EXEMPLUM_ADMIN_")
        .map_err(|_| "EXEMPLUM_ADMIN_DATABASE_URL is required")?;
    let pool = sqlx::PgPool::connect(config.database_url.expose())
        .await
        .map_err(|_| "Database connection failed")?;
    exemplum_dmn_users::repo::check_ready(&pool)
        .await
        .map_err(|_| "Database readiness check failed")?;
    let service = DefaultAccountService::new(Arc::new(PgAccountRepo::new(pool)));
    let Command::Promote { username } = command;
    service
        .promote_admin(&username)
        .await
        .map(|account| (account.id.to_string(), "promoted"))
        .map_err(|error| match error {
            AccountError::Repo(RepoError::NotFound) => {
                "No account has that username; sign in once after registering".into()
            }
            _ => "Administrator promotion failed".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_args(values: &[&str]) -> Result<Command, clap::Error> {
        parse(std::iter::once("exemplum-admin").chain(values.iter().copied()))
    }
    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
    #[test]
    fn promote_takes_one_valid_username() {
        assert_eq!(
            parse_args(&["promote", "alice"]).unwrap(),
            Command::Promote {
                username: "alice".into()
            }
        );
        for args in [
            &["promote"][..],
            &["promote", "Alice"][..],
            &["promote", " alice "][..],
            &["promote", "alice", "bob"][..],
            &["create", "--username", "alice"][..],
        ] {
            assert!(parse_args(args).is_err(), "{args:?}");
        }
    }
}
