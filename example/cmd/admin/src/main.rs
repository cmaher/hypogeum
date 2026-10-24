use exemplum_dmn_users::{accounts::AccountError, repo::PgAccountRepo};
use hypgm_envconfig::{EnvSource, Secret};
use serde::Deserialize;
use std::{
    io::{self, BufRead, Read},
    process::ExitCode,
};

#[derive(Deserialize)]
struct Config {
    database_url: Secret,
}

struct Input {
    username: String,
    display_name: String,
    stdin: bool,
}
fn parse(args: &[String]) -> Result<Input, &'static str> {
    let mut username = None;
    let mut display_name = None;
    let mut stdin = false;
    if args.first().map(String::as_str) != Some("bootstrap") {
        return Err(
            "Usage: exemplum-admin bootstrap --username NAME --display-name NAME [--password-stdin]",
        );
    }
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--username" if username.is_none() => username = args.next().cloned(),
            "--display-name" if display_name.is_none() => display_name = args.next().cloned(),
            "--password-stdin" if !stdin => stdin = true,
            _ => return Err("Invalid command arguments"),
        }
    }
    let username = username.ok_or("Username required")?;
    exemplum_dmn_users::accounts::normalize_username(&username).map_err(|_| "Invalid username")?;
    let display_name = display_name.ok_or("Display name required")?;
    exemplum_dmn_users::models::validate_name(display_name.trim())
        .map_err(|_| "Invalid display name")?;
    Ok(Input {
        username,
        display_name,
        stdin,
    })
}
fn read_password(mut reader: impl BufRead) -> Result<String, &'static str> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(1027)
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
    if !(12..=128).contains(&password.chars().count())
        || password.len() > 1024
        || password.contains('\0')
    {
        return Err("Password must contain 12 to 128 characters (at most 1024 bytes)");
    }
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["--describe"] || args == ["-h"] {
        println!(
            "{}",
            serde_json::json!({
                "commands": {"bootstrap": {"required": ["--username", "--display-name"], "optional": ["--password-stdin"]}},
                "configuration": "EXEMPLUM_ADMIN_DATABASE_URL (restricted exemplum_users role)",
                "output": "json",
                "password": "New accounts require a hidden prompt or one line on stdin; existing credentials are preserved"
            })
        );
        return ExitCode::SUCCESS;
    }

    match run().await {
        Ok(id) => {
            println!("{}", serde_json::json!({"ok":true,"account_id":id}));
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{}", serde_json::json!({"ok":false,"error":message}));
            ExitCode::FAILURE
        }
    }
}
async fn run() -> Result<String, &'static str> {
    let input = parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
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
    match repo
        .bootstrap_admin(&input.display_name, &input.username, None)
        .await
    {
        Ok(account) => return Ok(account.id.to_string()),
        Err(AccountError::Repo(hypgm_errors::RepoError::NotFound)) => {}
        Err(_) => return Err("Administrator bootstrap failed"),
    }
    let password = if input.stdin {
        read_password(io::stdin().lock())?
    } else {
        let password = rpassword::prompt_password("New administrator password: ")
            .map_err(|_| "Could not read password")?;
        validate_password(&password)?;
        password
    };
    let hash = tokio::task::spawn_blocking(move || hypgm_authn::password::hash_password(&password))
        .await
        .map_err(|_| "Password hashing failed")?
        .map_err(|_| "Password hashing failed")?;
    repo.bootstrap_admin(&input.display_name, &input.username, Some(&hash))
        .await
        .map(|account| account.id.to_string())
        .map_err(|_| "Administrator bootstrap failed")
}
#[cfg(test)]
mod tests {
    use super::*;
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
        let args = [
            "bootstrap",
            "--username",
            "admin",
            "--display-name",
            "Admin",
        ];
        assert!(parse(&args.map(str::to_owned)).is_ok());
        let mut args = args.map(str::to_owned).to_vec();
        args.extend(["--password".into(), "secret".into()]);
        assert_eq!(parse(&args).err(), Some("Invalid command arguments"));
    }
}
