use anyhow::{Context, Result};
use hypgm_env_config::{EnvSource, LogConfig};
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc};
use hypgm_dmn_users::{
    repo::{PgUserRepo, check_ready},
    server::GrpcUserServer,
    service::DefaultUserService,
};

fn default_bind() -> SocketAddr {
    "127.0.0.1:50051"
        .parse()
        .expect("valid default bind address")
}

#[derive(Debug, Deserialize)]
struct Config {
    database_url: hypgm_env_config::Secret,
    #[serde(default = "default_bind")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = EnvSource::load()?;
    let config: Config = env.deserialize("HYPOGEUM_USERS_")?;
    let log: LogConfig = env.deserialize("HYPOGEUM_")?;

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(&log.log)?)
        .init();

    let pool = postgres::create_pool(&postgres::PostgresConfig::new(config.database_url.expose()))
        .map_err(|_| anyhow::anyhow!("Invalid database configuration"))?;

    check_ready(&pool)
        .await
        .context("Users database is not ready; run explicit migrations")?;

    let service = Arc::new(DefaultUserService::new(Arc::new(PgUserRepo::new(
        pool.clone(),
    ))));

    tonic::transport::Server::builder()
        .add_service(GrpcUserServer::new(service).into_service())
        .serve_with_shutdown(config.bind, shutdown())
        .await
        .context("Users server failed")?;

    pool.close().await;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Install termination handler");
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
            "HYPOGEUM_USERS_DATABASE_URL",
            "postgres://user:pass@localhost/db",
        )]);
        let config: Config = env.deserialize("HYPOGEUM_USERS_").unwrap();
        assert_eq!(
            config.database_url.expose(),
            "postgres://user:pass@localhost/db"
        );
        assert_eq!(
            config.bind,
            "127.0.0.1:50051".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn test_config_missing_url() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let err = env.deserialize::<Config>("HYPOGEUM_USERS_").unwrap_err();
        assert!(err.to_string().contains("HYPOGEUM_USERS_DATABASE_URL"));
    }

    #[test]
    fn test_config_invalid_bind() {
        let env = EnvSource::from_pairs([
            (
                "HYPOGEUM_USERS_DATABASE_URL",
                "postgres://user:pass@localhost/db",
            ),
            ("HYPOGEUM_USERS_BIND", "not_a_valid_socket_addr"),
        ]);
        let err = env.deserialize::<Config>("HYPOGEUM_USERS_").unwrap_err();
        assert!(matches!(err, hypgm_env_config::ConfigError::Invalid(_)));
    }

    #[test]
    fn test_log_config_hypogeum_log() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let log: LogConfig = env.deserialize("HYPOGEUM_").unwrap();
        assert_eq!(log.log, "info");

        let env = EnvSource::from_pairs([("HYPOGEUM_LOG", "debug")]);
        let log: LogConfig = env.deserialize("HYPOGEUM_").unwrap();
        assert_eq!(log.log, "debug");
    }
}
