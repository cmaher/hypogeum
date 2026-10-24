use exemplum_dmn_users::{
    accounts::{AccountError, PASSWORD_MAX_BYTES},
    repo::PgAccountRepo,
};
use hypgm_envconfig::{EnvSource, Secret};
use hypgm_errors::RepoError;
use serde::Deserialize;
use std::{
    io::{self, BufRead, Read},
    process::ExitCode,
};

const USAGE: &str = "Usage: exemplum-admin create --username NAME (--display-name NAME [--password-stdin] | --promote)";

#[derive(Deserialize)]
struct Config {
    database_url: Secret,
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
fn parse(args: &[String]) -> Result<Command, &'static str> {
    let mut username = None;
    let mut display_name = None;
    let mut stdin = false;
    let mut promote = false;
    if args.first().map(String::as_str) != Some("create") {
        return Err(USAGE);
    }
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--username" if username.is_none() => username = args.next().cloned(),
            "--display-name" if display_name.is_none() => display_name = args.next().cloned(),
            "--password-stdin" if !stdin => stdin = true,
            "--promote" if !promote => promote = true,
            _ => return Err("Invalid command arguments"),
        }
    }
    let username = username.ok_or("Username required")?;
    exemplum_dmn_users::accounts::normalize_username(&username).map_err(|_| "Invalid username")?;
    if promote {
        // Promotion keeps the existing profile and credentials.
        if display_name.is_some() || stdin {
            return Err("--promote cannot be combined with --display-name or --password-stdin");
        }
        return Ok(Command::Promote { username });
    }
    let display_name = display_name.ok_or("Display name required")?;
    exemplum_dmn_users::models::validate_name(display_name.trim())
        .map_err(|_| "Invalid display name")?;
    Ok(Command::Create {
        username,
        display_name,
        stdin,
    })
}
fn read_password(mut reader: impl BufRead) -> Result<String, &'static str> {
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
fn validate_password(password: &str) -> Result<(), &'static str> {
    exemplum_dmn_users::accounts::validate_password(password)
        .map_err(|_| "Password must contain 12 to 128 characters without NUL")
}
#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["--describe"] || args == ["-h"] {
        println!(
            "{}",
            serde_json::json!({
                "commands": {"create": {
                    "required": ["--username"],
                    "modes": {
                        "create": {"required": ["--display-name"], "optional": ["--password-stdin"]},
                        "promote": {"required": ["--promote"]}
                    }
                }},
                "configuration": "EXEMPLUM_ADMIN_DATABASE_URL (restricted exemplum_users role)",
                "output": "json",
                "password": "New accounts require a hidden prompt or one line on stdin",
                "existing_usernames": "Rejected unless --promote is passed; promotion preserves credentials and profile"
            })
        );
        return ExitCode::SUCCESS;
    }

    match run().await {
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
async fn run() -> Result<(String, &'static str), &'static str> {
    let command = parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
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
    let repo = PgAccountRepo::new(pool);
    match command {
        Command::Promote { username } => repo
            .promote_admin(&username)
            .await
            .map(|account| (account.id.to_string(), "promoted"))
            .map_err(|error| match error {
                AccountError::Repo(RepoError::NotFound) => "No active account has that username",
                _ => "Administrator promotion failed",
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
            repo.create_admin(&display_name, &username, &hash)
                .await
                .map(|account| (account.id.to_string(), "created"))
                .map_err(|error| match error {
                    AccountError::DuplicateUsername => {
                        "Username already exists; pass --promote to make that account an administrator"
                    }
                    _ => "Administrator creation failed",
                })
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }
    #[test]
    fn passwords_are_bounded_and_preserve_spaces() {
        assert_eq!(
            read_password(&b"  secret phrase  \r\n"[..]).unwrap(),
            "  secret phrase  "
        );
        for password in [
            "short".to_owned(),
            "x".repeat(129),
            "a\0bcdefghijkl".to_owned(),
        ] {
            assert!(read_password(password.as_bytes()).is_err());
        }
        assert!(read_password("x".repeat(2000).as_bytes()).is_err());
    }
    #[test]
    fn arguments_never_accept_passwords() {
        let create = args(&["create", "--username", "admin", "--display-name", "Admin"]);
        assert!(parse(&create).is_ok());
        let mut with_password = create.clone();
        with_password.extend(args(&["--password", "secret"]));
        assert_eq!(
            parse(&with_password).err(),
            Some("Invalid command arguments")
        );
    }
    #[test]
    fn promotion_requires_an_explicit_flag_and_no_creation_inputs() {
        assert_eq!(
            parse(&args(&["create", "--username", "alice"])).err(),
            Some("Display name required")
        );
        assert_eq!(
            parse(&args(&["create", "--username", "alice", "--promote"])).unwrap(),
            Command::Promote {
                username: "alice".into()
            }
        );
        for extra in [&["--display-name", "Alice"][..], &["--password-stdin"][..]] {
            let mut promote = args(&["create", "--username", "alice", "--promote"]);
            promote.extend(args(extra));
            assert!(parse(&promote).is_err());
        }
        assert_eq!(
            parse(&args(&["bootstrap", "--username", "alice"])).err(),
            Some(USAGE)
        );
    }
}
