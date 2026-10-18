use hypgm_envconfig::Secret;
use hypgm_logging::LogFormat;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_sessions::SessionStore;

fn default_bind() -> SocketAddr {
    "127.0.0.1:8080"
        .parse()
        .expect("valid default bind address")
}

#[derive(Debug, Deserialize)]
struct AppConfig {
    #[serde(default = "default_bind")]
    bind: SocketAddr,
    database_url: Secret,
    #[serde(deserialize_with = "deserialize_signing_secret")]
    jwt_secret: Secret,
    #[serde(
        default = "default_max_password_verifications",
        deserialize_with = "deserialize_max_password_verifications"
    )]
    max_password_verifications: std::num::NonZeroUsize,
    #[serde(default = "default_session_cleanup_interval_seconds")]
    session_cleanup_interval_seconds: std::num::NonZeroU64,
    #[serde(default = "default_cedar_schema")]
    cedar_schema: PathBuf,
    #[serde(default = "default_cedar_policies")]
    cedar_policies: PathBuf,
}

fn default_max_password_verifications() -> std::num::NonZeroUsize {
    exemplum_app::PasswordConfig::default().max_verifications
}

fn deserialize_max_password_verifications<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<std::num::NonZeroUsize, D::Error> {
    let value = std::num::NonZeroUsize::deserialize(deserializer)?;
    if value.get() > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(serde::de::Error::custom(
            "Password verification limit exceeds semaphore capacity",
        ));
    }
    Ok(value)
}

fn default_session_cleanup_interval_seconds() -> std::num::NonZeroU64 {
    std::num::NonZeroU64::new(3600).expect("positive default")
}

fn default_cedar_schema() -> PathBuf {
    "config/schema.cedarschema".into()
}

fn default_cedar_policies() -> PathBuf {
    "config/policies.cedar".into()
}

fn deserialize_signing_secret<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Secret, D::Error> {
    let secret = Secret::deserialize(deserializer)?;
    if secret.expose().len() < 32 {
        return Err(serde::de::Error::custom(
            "JWT signing secret must contain at least 32 bytes",
        ));
    }
    Ok(secret)
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

    hypgm_authn::JwtValidator::from_hs256_secret(config.jwt_secret.expose().as_bytes())
        .context("Validate JWT signing configuration")?;

    let pool =
        hypgmx_rdbms::create_pool(&hypgmx_rdbms::PoolConfig::new(config.database_url.expose()))
            .context("Create application database pool")?;
    exemplum_dmn_users::repo::check_ready(&pool)
        .await
        .context("Check application database readiness; run explicit migrations first")?;
    let (schema, policies) = hypgm_authz::load_schema_and_policies_from_files(
        &config.cedar_schema,
        &config.cedar_policies,
    )
    .context("Load Cedar schema and policies")?;
    let components = exemplum_app::AppComponentsFactory::from_pool(
        pool.clone(),
        Arc::new(hypgm_authz::Authorizer::new(schema, policies)?),
        exemplum_app::TokenConfig::new(config.jwt_secret.expose().as_bytes().to_vec()),
        exemplum_app::PasswordConfig {
            max_verifications: config.max_password_verifications,
        },
    );
    // Check the actual runtime adapters before opening the listener; never migrate here.
    components
        .auth_repo
        .find_active_by_id(uuid::Uuid::nil())
        .await
        .context("Check authentication storage readiness")?;
    components
        .session_store
        .load(&tower_sessions::session::Id::default())
        .await
        .context("Check session storage readiness")?;

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .context("Bind app listener")?;

    tracing::info!(bind = %config.bind, "app listening");

    let session_store = components.session_store.clone();
    let mut background = tokio::task::JoinSet::new();
    background.spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            config.session_cleanup_interval_seconds.get(),
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = session_store.delete_expired().await {
                tracing::error!(%error, "Failed to clean up expired sessions");
            }
        }
    });
    let result = axum::serve(listener, exemplum_app::router(components))
        .with_graceful_shutdown(async {
            shutdown().await;
            tracing::info!("app shutting down");
        })
        .await;
    background.shutdown().await;
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

    #[test]
    fn bind_defaults_with_explicit_authentication_configuration() {
        let env = EnvSource::from_pairs([
            (
                "EXEMPLUM_APP_DATABASE_URL",
                "postgres://exemplum_users:password@localhost/db",
            ),
            (
                "EXEMPLUM_APP_JWT_SECRET",
                "a-test-signing-secret-with-at-least-32-bytes",
            ),
        ]);
        let config: AppConfig = env.deserialize("EXEMPLUM_APP_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:8080".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn bind_can_be_overridden_and_invalid_addresses_fail() {
        let env = EnvSource::from_pairs([
            ("EXEMPLUM_APP_BIND", "127.0.0.1:9090"),
            (
                "EXEMPLUM_APP_DATABASE_URL",
                "postgres://exemplum_users:password@localhost/db",
            ),
            (
                "EXEMPLUM_APP_JWT_SECRET",
                "a-test-signing-secret-with-at-least-32-bytes",
            ),
        ]);
        let config: AppConfig = env.deserialize("EXEMPLUM_APP_").unwrap();
        assert_eq!(config.bind, "127.0.0.1:9090".parse::<SocketAddr>().unwrap());
        let env = EnvSource::from_pairs([
            ("EXEMPLUM_APP_BIND", "invalid"),
            (
                "EXEMPLUM_APP_DATABASE_URL",
                "postgres://exemplum_users:password@localhost/db",
            ),
            (
                "EXEMPLUM_APP_JWT_SECRET",
                "a-test-signing-secret-with-at-least-32-bytes",
            ),
        ]);
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

#[cfg(test)]
mod configuration_regressions {
    use super::AppConfig;
    use hypgm_envconfig::EnvSource;
    #[test]
    fn startup_requires_database_credentials_and_a_signing_secret() {
        for pairs in [
            vec![],
            vec![(
                "EXEMPLUM_APP_DATABASE_URL",
                "postgres://exemplum_users:password@localhost/db",
            )],
            vec![(
                "EXEMPLUM_APP_JWT_SECRET",
                "a-configured-secret-with-32-bytes-or-more",
            )],
        ] {
            assert!(
                EnvSource::from_pairs(pairs)
                    .deserialize::<AppConfig>("EXEMPLUM_APP_")
                    .is_err()
            );
        }
    }
    #[test]
    fn startup_validates_signing_secret_length() {
        for length in [0, 1, 31, 32, 64] {
            let secret = "a".repeat(length);
            let env = EnvSource::from_pairs([
                (
                    "EXEMPLUM_APP_DATABASE_URL",
                    "postgres://exemplum_users:password@localhost/db",
                ),
                ("EXEMPLUM_APP_JWT_SECRET", secret.as_str()),
            ]);
            assert_eq!(
                env.deserialize::<AppConfig>("EXEMPLUM_APP_").is_ok(),
                length >= 32
            );
        }
    }
}

#[cfg(test)]
mod resource_configuration_tests {
    #[test]
    fn zero_resource_settings_are_rejected() {
        for key in [
            "EXEMPLUM_APP_MAX_PASSWORD_VERIFICATIONS",
            "EXEMPLUM_APP_SESSION_CLEANUP_INTERVAL_SECONDS",
        ] {
            let env = hypgm_envconfig::EnvSource::from_pairs([
                (
                    "EXEMPLUM_APP_DATABASE_URL",
                    "postgres://user:pass@localhost/db",
                ),
                (
                    "EXEMPLUM_APP_JWT_SECRET",
                    "private-test-secret-with-at-least-32-bytes",
                ),
                (key, "0"),
            ]);
            assert!(
                env.deserialize::<super::AppConfig>("EXEMPLUM_APP_")
                    .is_err(),
                "{key} must be positive"
            );
        }
    }

    #[test]
    fn resource_settings_have_defaults_and_accept_overrides() {
        let base = [
            (
                "EXEMPLUM_APP_DATABASE_URL",
                "postgres://user:pass@localhost/db",
            ),
            (
                "EXEMPLUM_APP_JWT_SECRET",
                "private-test-secret-with-at-least-32-bytes",
            ),
        ];
        let defaults: super::AppConfig = hypgm_envconfig::EnvSource::from_pairs(base)
            .deserialize("EXEMPLUM_APP_")
            .unwrap();
        assert_eq!(defaults.max_password_verifications.get(), 4);
        assert_eq!(defaults.session_cleanup_interval_seconds.get(), 3600);
        let env = hypgm_envconfig::EnvSource::from_pairs(base.into_iter().chain([
            ("EXEMPLUM_APP_MAX_PASSWORD_VERIFICATIONS", "2"),
            ("EXEMPLUM_APP_SESSION_CLEANUP_INTERVAL_SECONDS", "15"),
        ]));
        let config: super::AppConfig = env.deserialize("EXEMPLUM_APP_").unwrap();
        assert_eq!(config.max_password_verifications.get(), 2);
        assert_eq!(config.session_cleanup_interval_seconds.get(), 15);
        let too_large = (tokio::sync::Semaphore::MAX_PERMITS + 1).to_string();
        let env = hypgm_envconfig::EnvSource::from_pairs(base.into_iter().chain([(
            "EXEMPLUM_APP_MAX_PASSWORD_VERIFICATIONS",
            too_large.as_str(),
        )]));
        assert!(
            env.deserialize::<super::AppConfig>("EXEMPLUM_APP_")
                .is_err()
        );
    }
}
