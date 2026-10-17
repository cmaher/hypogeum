use anyhow::{Context, Result};
use hypgm_env_config::EnvSource;
use serde::Deserialize;
use std::{net::SocketAddr, time::Duration};
use hypgm_ext_tasks::board::Board;

fn default_board_bind() -> SocketAddr {
    "127.0.0.1:8082"
        .parse()
        .expect("valid default board bind address")
}

fn default_shutdown_seconds() -> u64 {
    30
}

#[derive(Debug, Deserialize)]
struct Config {
    database_url: hypgm_env_config::Secret,
    #[serde(default = "default_board_bind")]
    board_bind: SocketAddr,
    #[serde(default = "default_shutdown_seconds")]
    shutdown_seconds: u64,
}

impl Config {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (1..=3600).contains(&self.shutdown_seconds),
            "Shutdown must be 1..3600 seconds"
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = EnvSource::load()?;
    let config: Config = env.deserialize("HYPOGEUM_TASKS_")?;
    config.validate()?;

    let pool = sqlx::PgPool::connect(config.database_url.expose())
        .await
        .map_err(|_| anyhow::anyhow!("Tasks connection failed (URL redacted)"))?;
    hypgm_ext_tasks::check_ready(&pool).await?;
    let listener = tokio::net::TcpListener::bind(config.board_bind)
        .await
        .context("Cannot bind Board")?;
    // No production queues or handlers are registered by this infrastructure host.
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, Board::new().router()).with_graceful_shutdown(async {
        let _ = stopped.await;
    });
    let serving = std::future::IntoFuture::into_future(server);
    tokio::pin!(serving);
    tokio::select! {
        result = &mut serving => { result.context("Board server failed")?; }
        () = shutdown_signal() => {
            let _ = stop.send(());
            tokio::time::timeout(Duration::from_secs(config.shutdown_seconds), &mut serving).await.context("Board drain deadline exceeded")??;
        }
    }
    pool.close().await;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install termination signal handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypgm_env_config::EnvSource;

    #[test]
    fn test_config_defaults() {
        let env = EnvSource::from_pairs([(
            "HYPOGEUM_TASKS_DATABASE_URL",
            "postgres://user:pass@localhost/db",
        )]);
        let config: Config = env.deserialize("HYPOGEUM_TASKS_").unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.database_url.expose(),
            "postgres://user:pass@localhost/db"
        );
        assert_eq!(
            config.board_bind,
            "127.0.0.1:8082".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.shutdown_seconds, 30);
    }

    #[test]
    fn test_config_missing_url_names_hypogeum_tasks_database_url() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let err = env.deserialize::<Config>("HYPOGEUM_TASKS_").unwrap_err();
        assert!(err.to_string().contains("HYPOGEUM_TASKS_DATABASE_URL"));
    }

    #[test]
    fn test_shutdown_seconds_validation() {
        for invalid_val in [0, 3601] {
            let env = EnvSource::from_pairs([
                (
                    "HYPOGEUM_TASKS_DATABASE_URL",
                    "postgres://user:pass@localhost/db",
                ),
                ("HYPOGEUM_TASKS_SHUTDOWN_SECONDS", &invalid_val.to_string()),
            ]);
            let config: Config = env.deserialize("HYPOGEUM_TASKS_").unwrap();
            let err = config.validate().unwrap_err();
            assert_eq!(err.to_string(), "Shutdown must be 1..3600 seconds");
        }

        for valid_val in [1, 3600] {
            let env = EnvSource::from_pairs([
                (
                    "HYPOGEUM_TASKS_DATABASE_URL",
                    "postgres://user:pass@localhost/db",
                ),
                ("HYPOGEUM_TASKS_SHUTDOWN_SECONDS", &valid_val.to_string()),
            ]);
            let config: Config = env.deserialize("HYPOGEUM_TASKS_").unwrap();
            assert!(config.validate().is_ok());
        }
    }
}
