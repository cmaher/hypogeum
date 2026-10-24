use anyhow::Context;
use hypgm_envconfig::{EnvSource, LogConfig, Secret};
use hypgm_logging::LogFormat;
use hypgmx_tasks::{WorkerConfig, worker::Lifecycle};
use serde::Deserialize;
use std::{num::NonZeroU32, sync::Arc, time::Duration};

#[derive(Debug, Deserialize)]
#[serde(default)]
struct TasksConfig {
    database_url: Secret,
    handler_database_url: Secret,
    concurrency: usize,
    max_attempts: usize,
    backoff_initial_seconds: u64,
    backoff_max_seconds: u64,
    shutdown_seconds: u64,
    heartbeat_seconds: u64,
    missed_heartbeats: usize,
    pool_connections: NonZeroU32,
    handler_pool_connections: NonZeroU32,
    connect_timeout_seconds: u64,
}

impl Default for TasksConfig {
    fn default() -> Self {
        let worker = WorkerConfig::default();
        Self {
            database_url: Secret::new(String::new()),
            handler_database_url: Secret::new(String::new()),
            concurrency: worker.concurrency,
            max_attempts: worker.max_attempts,
            backoff_initial_seconds: worker.backoff_initial.as_secs(),
            backoff_max_seconds: worker.backoff_max.as_secs(),
            shutdown_seconds: worker.shutdown_timeout.as_secs(),
            heartbeat_seconds: worker.heartbeat_interval.as_secs(),
            missed_heartbeats: worker.missed_heartbeats,
            pool_connections: NonZeroU32::new(4).unwrap(),
            handler_pool_connections: NonZeroU32::new(4).unwrap(),
            connect_timeout_seconds: 5,
        }
    }
}

impl TasksConfig {
    fn worker_config(&self) -> anyhow::Result<WorkerConfig> {
        anyhow::ensure!(
            !self.database_url.expose().is_empty()
                && !self.handler_database_url.expose().is_empty(),
            "Both tasks and handler database URLs are required"
        );
        anyhow::ensure!(
            self.connect_timeout_seconds > 0,
            "Connection timeout must be positive"
        );
        for seconds in [self.shutdown_seconds, self.connect_timeout_seconds] {
            anyhow::ensure!(
                std::time::Instant::now()
                    .checked_add(Duration::from_secs(seconds))
                    .is_some(),
                "Timeout exceeds the supported clock range"
            );
        }
        let config = WorkerConfig {
            concurrency: self.concurrency,
            max_attempts: self.max_attempts,
            backoff_initial: Duration::from_secs(self.backoff_initial_seconds),
            backoff_max: Duration::from_secs(self.backoff_max_seconds),
            shutdown_timeout: Duration::from_secs(self.shutdown_seconds),
            heartbeat_interval: Duration::from_secs(self.heartbeat_seconds),
            missed_heartbeats: self.missed_heartbeats,
            tasks_role: "exemplum_tasks".into(),
        };
        config.validate()?;
        Ok(config)
    }
}

#[derive(Deserialize)]
struct LoggingConfig {
    #[serde(default)]
    log_format: LogFormat,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env = EnvSource::load_for("EXEMPLUM")?;
    let config: TasksConfig = env.deserialize("EXEMPLUM_TASKS_")?;
    let policy = config.worker_config()?;
    let log: LogConfig = env.deserialize("EXEMPLUM_")?;
    let logging: LoggingConfig = env.deserialize("EXEMPLUM_")?;
    hypgm_logging::init(&log.log, logging.log_format).context("Initialize logging")?;

    // Parse errors are deliberately replaced so URLs and credentials cannot escape.
    let connection = config
        .database_url
        .expose()
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid tasks database URL"))?;
    let lifecycle = Lifecycle::new(
        "exemplum-welcome",
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.pool_connections.get())
            .acquire_timeout(Duration::from_secs(config.connect_timeout_seconds)),
        connection,
    );
    let handler_pool = hypgmx_rdbms::create_pool(
        &hypgmx_rdbms::PoolConfig::new(config.handler_database_url.expose())
            .with_max_connections(config.handler_pool_connections.get())
            .with_connect_timeout(Duration::from_secs(config.connect_timeout_seconds)),
    )
    .map_err(|_| anyhow::anyhow!("Invalid handler database configuration"))?;
    exemplum_dmn_users::repo::check_ready(&handler_pool)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Handler database is not ready; check users credentials and run explicit migrations"
            )
        })?;
    let queue = exemplum_dmn_users::jobs::welcome_queue(lifecycle.pool().clone(), policy.clone())?;
    let repo: Arc<dyn exemplum_dmn_users::accounts::WelcomeRepo> = Arc::new(
        exemplum_dmn_users::repo::PgAccountRepo::new(handler_pool.clone()),
    );
    let worker = queue
        .worker(&lifecycle)
        .await
        .context(
            "Queue database is not ready; check tasks credentials and run explicit migrations",
        )?
        .build(move |job: exemplum_dmn_users::jobs::WelcomeJob| {
            exemplum_dmn_users::jobs::handle_welcome(job, repo.clone())
        });
    tracing::info!(
        queue = exemplum_dmn_users::jobs::WELCOME_QUEUE,
        "tasks starting"
    );
    let result = lifecycle
        .run_until(worker.run(), shutdown(), policy.shutdown_timeout)
        .await;
    // The worker has finished or been dropped, so handler connections are idle.
    tokio::time::timeout(policy.shutdown_timeout, handler_pool.close())
        .await
        .context("Timed out closing handler database pool")?;
    result.context("Welcome worker failed")?;
    tracing::info!("tasks stopped");
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
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(extra: &[(&str, &str)]) -> anyhow::Result<TasksConfig> {
        let env = EnvSource::from_pairs(
            [
                (
                    "EXEMPLUM_TASKS_DATABASE_URL",
                    "postgres://queue:private@localhost/db",
                ),
                (
                    "EXEMPLUM_TASKS_HANDLER_DATABASE_URL",
                    "postgres://users:private@localhost/db",
                ),
            ]
            .into_iter()
            .chain(extra.iter().copied()),
        );
        Ok(env.deserialize("EXEMPLUM_TASKS_")?)
    }

    #[test]
    fn missing_credentials_and_invalid_operational_limits_fail() {
        let empty: TasksConfig = EnvSource::from_pairs(Vec::<(&str, &str)>::new())
            .deserialize("EXEMPLUM_TASKS_")
            .unwrap();
        assert!(empty.worker_config().is_err());
        for key in [
            "CONCURRENCY",
            "MAX_ATTEMPTS",
            "SHUTDOWN_SECONDS",
            "HEARTBEAT_SECONDS",
            "MISSED_HEARTBEATS",
            "CONNECT_TIMEOUT_SECONDS",
            "POOL_CONNECTIONS",
            "HANDLER_POOL_CONNECTIONS",
        ] {
            let key = format!("EXEMPLUM_TASKS_{key}");
            assert!(
                config(&[(&key, "0")])
                    .and_then(|c| c.worker_config())
                    .is_err(),
                "{key}"
            );
        }
        assert!(
            config(&[("EXEMPLUM_TASKS_BACKOFF_MAX_SECONDS", "1")])
                .unwrap()
                .worker_config()
                .is_err()
        );
    }

    #[test]
    fn configuration_overrides_and_redacts_credentials() {
        let config = config(&[
            ("EXEMPLUM_TASKS_CONCURRENCY", "3"),
            ("EXEMPLUM_TASKS_SHUTDOWN_SECONDS", "9"),
        ])
        .unwrap();
        let policy = config.worker_config().unwrap();
        assert_eq!(policy.concurrency, 3);
        assert_eq!(policy.shutdown_timeout, Duration::from_secs(9));
        assert!(!format!("{config:?}").contains("private"));
    }
}
