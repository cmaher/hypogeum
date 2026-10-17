use hypgm_logging::LogFormat;
use serde::Deserialize;
use std::net::SocketAddr;

fn default_bind() -> SocketAddr {
    "127.0.0.1:8080"
        .parse()
        .expect("valid default bind address")
}

#[derive(Debug, Deserialize)]
struct AppConfig {
    #[serde(default = "default_bind")]
    bind: SocketAddr,
}

#[derive(Debug, Deserialize)]
struct AppLogConfig {
    #[serde(default)]
    log_format: LogFormat,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use hypgm_dmn_hello_world::service::DefaultHelloWorldService;
    use hypgm_env_config::{EnvSource, LogConfig};
    use std::sync::Arc;

    let env = EnvSource::load()?;
    let config: AppConfig = env.deserialize("HYPOGEUM_APP_")?;
    let log: LogConfig = env.deserialize("HYPOGEUM_")?;
    let log_format: AppLogConfig = env.deserialize("HYPOGEUM_")?;

    hypgm_logging::init(&log.log, log_format.log_format).context("Initialize logging subscriber")?;

    tracing::info!(
        bind = %config.bind,
        format = %log_format.log_format,
        "app starting"
    );

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .context("Bind app listener")?;

    tracing::info!(bind = %config.bind, "app listening");

    axum::serve(listener, hypgm_dmn_app::router(Arc::new(DefaultHelloWorldService)))
        .with_graceful_shutdown(async {
            shutdown().await;
            tracing::info!("app shutting down");
        })
        .await
        .context("App server failed")?;

    tracing::info!("app stopped");
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Install termination handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {}
        }
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
    fn bind_defaults_without_database_configuration() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let config: AppConfig = env.deserialize("HYPOGEUM_APP_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:8080".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn bind_can_be_overridden_and_invalid_addresses_fail() {
        let env = EnvSource::from_pairs([("HYPOGEUM_APP_BIND", "127.0.0.1:9090")]);
        let config: AppConfig = env.deserialize("HYPOGEUM_APP_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:9090".parse::<SocketAddr>().unwrap());
        let env = EnvSource::from_pairs([("HYPOGEUM_APP_BIND", "invalid")]);
        assert!(env.deserialize::<AppConfig>("HYPOGEUM_APP_").is_err());
    }

    #[test]
    fn log_format_defaults_and_overrides() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let log: AppLogConfig = env.deserialize("HYPOGEUM_").unwrap();
        assert_eq!(log.log_format, LogFormat::Text);

        let env = EnvSource::from_pairs([("HYPOGEUM_LOG_FORMAT", "json")]);
        let log: AppLogConfig = env.deserialize("HYPOGEUM_").unwrap();
        assert_eq!(log.log_format, LogFormat::Json);

        let env = EnvSource::from_pairs([("HYPOGEUM_LOG_FORMAT", "invalid")]);
        assert!(env.deserialize::<AppLogConfig>("HYPOGEUM_").is_err());
    }
}
