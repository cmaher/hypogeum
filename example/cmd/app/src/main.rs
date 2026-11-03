use hypgm_envconfig::Secret;
use hypgm_logging::LogFormat;
use serde::Deserialize;
use std::net::SocketAddr;
use std::num::NonZeroU64;

fn default_bind() -> SocketAddr {
    "127.0.0.1:3000"
        .parse()
        .expect("valid default bind address")
}

fn default_kratos_public_url() -> String {
    "http://127.0.0.1:4433".into()
}

fn default_kratos_browser_url() -> String {
    "http://localhost:8080/.ory/kratos".into()
}

fn default_kratos_timeout_seconds() -> NonZeroU64 {
    NonZeroU64::new(hypgmx_kratos::DEFAULT_REQUEST_TIMEOUT.as_secs()).expect("positive default")
}

#[derive(Debug, Deserialize)]
struct AppConfig {
    #[serde(default = "default_bind")]
    bind: SocketAddr,
    database_url: Secret,
    /// Kratos public API as reached by the app.
    #[serde(default = "default_kratos_public_url")]
    kratos_public_url: String,
    /// Kratos public API as reached by browsers (through Caddy).
    #[serde(default = "default_kratos_browser_url")]
    kratos_browser_url: String,
    #[serde(default = "default_kratos_timeout_seconds")]
    kratos_timeout_seconds: NonZeroU64,
}

#[derive(Debug, Deserialize)]
struct AppLogConfig {
    #[serde(default)]
    log_format: LogFormat,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use hypgm_envconfig::{EnvSource, LogConfig};

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

    let kratos = hypgmx_kratos::KratosClient::new(hypgmx_kratos::KratosConfig {
        public_url: config
            .kratos_public_url
            .parse()
            .context("Parse EXEMPLUM_APP_KRATOS_PUBLIC_URL")?,
        request_timeout: std::time::Duration::from_secs(config.kratos_timeout_seconds.get()),
    })
    .context("Configure Kratos client")?;
    let browser_url = exemplum_app::BrowserUrl::new(&config.kratos_browser_url)
        .context("Parse EXEMPLUM_APP_KRATOS_BROWSER_URL")?;

    let pool =
        hypgmx_rdbms::create_pool(&hypgmx_rdbms::PoolConfig::new(config.database_url.expose()))
            .context("Create application database pool")?;
    // Check dependencies before opening the listener; never migrate here.
    exemplum_dmn_users::repo::check_ready(&pool)
        .await
        .context("Check application database readiness; run explicit migrations first")?;
    kratos
        .ready()
        .await
        .context("Check Kratos readiness; start it with `make auth-up`")?;
    let components =
        exemplum_app::AppComponentsFactory::from_pool(pool.clone(), kratos, browser_url);

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .context("Bind app listener")?;

    tracing::info!(bind = %config.bind, "app listening");

    let result = axum::serve(listener, exemplum_app::router(components))
        .with_graceful_shutdown(async {
            shutdown().await;
            tracing::info!("app shutting down");
        })
        .await;
    pool.close().await;
    result.context("App server failed")?;
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
    use hypgm_envconfig::EnvSource;

    const DATABASE_URL: (&str, &str) = (
        "EXEMPLUM_APP_DATABASE_URL",
        "postgres://exemplum_users:password@localhost/db",
    );

    #[test]
    fn defaults_need_only_database_credentials() {
        assert!(
            EnvSource::from_pairs(Vec::<(&str, &str)>::new())
                .deserialize::<AppConfig>("EXEMPLUM_APP_")
                .is_err()
        );
        let config: AppConfig = EnvSource::from_pairs([DATABASE_URL])
            .deserialize("EXEMPLUM_APP_")
            .unwrap();
        assert_eq!(config.bind, "127.0.0.1:3000".parse::<SocketAddr>().unwrap());
        assert_eq!(config.kratos_public_url, "http://127.0.0.1:4433");
        assert_eq!(
            config.kratos_browser_url,
            "http://localhost:8080/.ory/kratos"
        );
        assert_eq!(config.kratos_timeout_seconds.get(), 5);
    }

    #[test]
    fn settings_can_be_overridden_and_invalid_values_fail() {
        let config: AppConfig = EnvSource::from_pairs([
            DATABASE_URL,
            ("EXEMPLUM_APP_BIND", "127.0.0.1:9090"),
            ("EXEMPLUM_APP_KRATOS_PUBLIC_URL", "http://kratos:4433"),
            (
                "EXEMPLUM_APP_KRATOS_BROWSER_URL",
                "http://app.test/.ory/kratos",
            ),
            ("EXEMPLUM_APP_KRATOS_TIMEOUT_SECONDS", "2"),
        ])
        .deserialize("EXEMPLUM_APP_")
        .unwrap();
        assert_eq!(config.bind, "127.0.0.1:9090".parse::<SocketAddr>().unwrap());
        assert_eq!(config.kratos_public_url, "http://kratos:4433");
        assert_eq!(config.kratos_browser_url, "http://app.test/.ory/kratos");
        assert_eq!(config.kratos_timeout_seconds.get(), 2);
        for (key, value) in [
            ("EXEMPLUM_APP_BIND", "invalid"),
            ("EXEMPLUM_APP_KRATOS_TIMEOUT_SECONDS", "0"),
        ] {
            assert!(
                EnvSource::from_pairs([DATABASE_URL, (key, value)])
                    .deserialize::<AppConfig>("EXEMPLUM_APP_")
                    .is_err(),
                "{key}={value}"
            );
        }
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
