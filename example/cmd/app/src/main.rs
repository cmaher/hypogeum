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
    use exemplum_dmn_hello_world::service::DefaultHelloWorldService;
    use hypgm_env_config::{EnvSource, LogConfig};
    use std::sync::Arc;

    let env = EnvSource::load_for("EXEMPLUM")?;
    let config: AppConfig = env.deserialize("EXEMPLUM_APP_")?;
    let log: LogConfig = env.deserialize("EXEMPLUM_")?;
    let log_format: AppLogConfig = env.deserialize("EXEMPLUM_")?;

    hypgm_logging::init(&log.log, log_format.log_format)
        .context("Initialize logging subscriber")?;

    tracing::info!(
        bind = %config.bind,
        format = %log_format.log_format,
        "app starting"
    );

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .context("Bind app listener")?;

    tracing::info!(bind = %config.bind, "app listening");

    axum::serve(
        listener,
        exemplum_app::router(Arc::new(DefaultHelloWorldService)),
    )
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
        let config: AppConfig = env.deserialize("EXEMPLUM_APP_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:8080".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn bind_can_be_overridden_and_invalid_addresses_fail() {
        let env = EnvSource::from_pairs([("EXEMPLUM_APP_BIND", "127.0.0.1:9090")]);
        let config: AppConfig = env.deserialize("EXEMPLUM_APP_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:9090".parse::<SocketAddr>().unwrap());
        let env = EnvSource::from_pairs([("EXEMPLUM_APP_BIND", "invalid")]);
        assert!(env.deserialize::<AppConfig>("EXEMPLUM_APP_").is_err());
    }

    #[test]
    fn log_format_defaults_and_overrides() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let log: AppLogConfig = env.deserialize("EXEMPLUM_").unwrap();
        assert_eq!(log.log_format, LogFormat::Text);

        let env = EnvSource::from_pairs([("EXEMPLUM_LOG_FORMAT", "json")]);
        let log: AppLogConfig = env.deserialize("EXEMPLUM_").unwrap();
        assert_eq!(log.log_format, LogFormat::Json);

        let env = EnvSource::from_pairs([("EXEMPLUM_LOG_FORMAT", "invalid")]);
        assert!(env.deserialize::<AppLogConfig>("EXEMPLUM_").is_err());
    }
}
