use clap::{ArgGroup, Args, Parser, Subcommand, error::ErrorKind};
use exemplum_dmn_users::{
    accounts::{AccountError, AdminBootstrapService, DefaultAccountService, PASSWORD_MAX_BYTES},
    repo::PgAccountRepo,
};
use hypgm_envconfig::{EnvSource, Secret};
use hypgm_errors::RepoError;
use serde::Deserialize;
use std::{
    io::{self, BufRead, Read},
    process::ExitCode,
    sync::Arc,
};

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
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Create a new administrator, or promote an existing account with --promote.
    Create(CreateArgs),
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("mode").required(true).args(["display_name", "promote"])))]
struct CreateArgs {
    /// Login username for the administrator.
    #[arg(long, value_parser = parse_username)]
    username: String,
    /// Display name for a new administrator; an existing username is rejected.
    #[arg(long, value_parser = parse_display_name)]
    display_name: Option<String>,
    /// Read the new password as one line from stdin instead of prompting.
    #[arg(long, requires = "display_name", conflicts_with = "promote")]
    password_stdin: bool,
    /// Promote the existing account that owns the username, keeping its
    /// credentials and profile.
    #[arg(long)]
    promote: bool,
}

fn parse_username(value: &str) -> Result<String, &'static str> {
    exemplum_dmn_users::accounts::normalize_username(value)
        .map(str::to_owned)
        .map_err(|_| "invalid username")
}
fn parse_display_name(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    exemplum_dmn_users::models::validate_name(value).map_err(|_| "invalid display name")?;
    Ok(value.to_owned())
}

#[derive(Debug, PartialEq)]
enum Command {
    /// Create a new administrator; an existing username is a hard error.
    Create {
        username: String,
        display_name: String,
        stdin: bool,
    },
    /// Explicitly promote the existing account that owns the username.
    Promote { username: String },
}
impl From<CreateArgs> for Command {
    fn from(args: CreateArgs) -> Self {
        // The required `mode` group guarantees exactly one of the two.
        match args.display_name {
            Some(display_name) => Self::Create {
                username: args.username,
                display_name,
                stdin: args.password_stdin,
            },
            None => Self::Promote {
                username: args.username,
            },
        }
    }
}
fn parse<I, T>(args: I) -> Result<Command, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let CliCommand::Create(args) = Cli::try_parse_from(args)?.command;
    Ok(args.into())
}
fn read_password(mut reader: impl BufRead) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        // Read past the byte limit (plus a line ending) so oversized input is rejected.
        .take(PASSWORD_MAX_BYTES as u64 + 3)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "Could not read password")?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    let password = String::from_utf8(bytes).map_err(|_| "Invalid password")?;
    validate_password(&password)?;
    Ok(password)
}
fn validate_password(password: &str) -> Result<(), String> {
    exemplum_dmn_users::accounts::validate_password(password)
        .map_err(|_| exemplum_dmn_users::accounts::password_policy())
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
    match command {
        Command::Promote { username } => service
            .promote_admin(&username)
            .await
            .map(|account| (account.id.to_string(), "promoted"))
            .map_err(|error| match error {
                AccountError::Repo(RepoError::NotFound) => {
                    "No active account has that username".into()
                }
                _ => "Administrator promotion failed".into(),
            }),
        Command::Create {
            username,
            display_name,
            stdin,
        } => {
            let password = if stdin {
                read_password(io::stdin().lock())?
            } else {
                let password = rpassword::prompt_password("New administrator password: ")
                    .map_err(|_| "Could not read password")?;
                validate_password(&password)?;
                password
            };
            let hash = tokio::task::spawn_blocking(move || {
                hypgm_authn::password::hash_password(&password)
            })
            .await
            .map_err(|_| "Password hashing failed")?
            .map_err(|_| "Password hashing failed")?;
            service
                .create_admin(&display_name, &username, &hash)
                .await
                .map(|account| (account.id.to_string(), "created"))
                .map_err(|error| match error {
                    AccountError::DuplicateUsername => {
                        "Username already exists; pass --promote to make that account an administrator".into()
                    }
                    _ => "Administrator creation failed".into(),
                })
        }
    }
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
    fn passwords_are_bounded_and_preserve_spaces() {
        assert_eq!(
            read_password(&b"  secret phrase  \r\n"[..]).unwrap(),
            "  secret phrase  "
        );
        assert_eq!(read_password(&b"eight ch\n"[..]).unwrap(), "eight ch");
        for password in [
            "short".to_owned(),
            "x".repeat(129),
            "a\0bcdefghijkl".to_owned(),
        ] {
            assert!(read_password(password.as_bytes()).is_err());
        }
        assert!(read_password("x".repeat(2000).as_bytes()).is_err());
        assert_eq!(
            read_password(&b"short\n"[..]).unwrap_err(),
            "Password must contain 8 to 128 characters without NUL"
        );
    }
    #[test]
    fn arguments_never_accept_passwords() {
        let create = ["create", "--username", "admin", "--display-name", "Admin"];
        assert_eq!(
            parse_args(&create).unwrap(),
            Command::Create {
                username: "admin".into(),
                display_name: "Admin".into(),
                stdin: false,
            }
        );
        let mut with_password = create.to_vec();
        with_password.extend(["--password", "secret"]);
        assert_eq!(
            parse_args(&with_password).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
    }
    #[test]
    fn flag_values_are_never_swallowed_as_option_values() {
        for args in [
            &[
                "create",
                "--username",
                "admin",
                "--display-name",
                "--password-stdin",
            ][..],
            &["create", "--username", "--promote"][..],
            &["create", "--username"][..],
            &["create", "--username", "a", "--username", "b", "--promote"][..],
            &["create", "--username", " ", "--promote"][..],
        ] {
            assert!(parse_args(args).is_err(), "{args:?}");
        }
    }
    #[test]
    fn promotion_requires_an_explicit_flag_and_no_creation_inputs() {
        assert_eq!(
            parse_args(&["create", "--username", "alice"])
                .unwrap_err()
                .kind(),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            parse_args(&["create", "--username", " alice ", "--promote"]).unwrap(),
            Command::Promote {
                username: "alice".into()
            }
        );
        for extra in [&["--display-name", "Alice"][..], &["--password-stdin"][..]] {
            let mut promote = vec!["create", "--username", "alice", "--promote"];
            promote.extend(extra);
            assert!(parse_args(&promote).is_err());
        }
        assert_eq!(
            parse_args(&["bootstrap", "--username", "alice"])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidSubcommand
        );
    }
}
