use serde::Deserialize;
use std::net::SocketAddr;

fn default_bind() -> SocketAddr {
    "127.0.0.1:8080"
        .parse()
        .expect("valid default bind address")
}

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_bind")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use env_config::{EnvSource, LogConfig};
    use hello_world::service::DefaultHelloWorldService;
    use std::sync::Arc;

    let env = EnvSource::load()?;
    let config: Config = env.deserialize("HYPOGEUM_WEB_")?;
    let log: LogConfig = env.deserialize("HYPOGEUM_")?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(&log.log)?)
        .init();
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .context("Bind public web listener")?;
    axum::serve(
        listener,
        web_server::router(Arc::new(DefaultHelloWorldService)),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .context("Public web server failed")?;
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
    use env_config::EnvSource;

    #[test]
    fn bind_defaults_without_database_configuration() {
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let config: Config = env.deserialize("HYPOGEUM_WEB_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:8080".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn bind_can_be_overridden_and_invalid_addresses_fail() {
        let env = EnvSource::from_pairs([("HYPOGEUM_WEB_BIND", "127.0.0.1:9090")]);
        let config: Config = env.deserialize("HYPOGEUM_WEB_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:9090".parse::<SocketAddr>().unwrap());
        let env = EnvSource::from_pairs([("HYPOGEUM_WEB_BIND", "invalid")]);
        assert!(env.deserialize::<Config>("HYPOGEUM_WEB_").is_err());
    }
}
