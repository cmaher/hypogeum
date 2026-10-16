use serde::{Deserialize, Serialize};
pub use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostgresConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
}

impl PostgresConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: 10,
            min_connections: 1,
            connect_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(600),
        }
    }

    pub fn with_max_connections(mut self, max: u32) -> Self {
        self.max_connections = max;
        self
    }

    pub fn with_min_connections(mut self, min: u32) -> Self {
        self.min_connections = min;
        self
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
}

pub fn pool_options(config: &PostgresConfig) -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.connect_timeout)
        .idle_timeout(Some(config.idle_timeout))
}

pub fn create_pool(config: &PostgresConfig) -> Result<PgPool, sqlx::Error> {
    pool_options(config).connect_lazy(&config.url)
}

pub async fn ping(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_construction() {
        let config = PostgresConfig::new("postgres://user:pass@localhost:5432/testdb")
            .with_max_connections(20)
            .with_min_connections(2)
            .with_connect_timeout(Duration::from_secs(10))
            .with_idle_timeout(Duration::from_secs(300));

        assert_eq!(config.url, "postgres://user:pass@localhost:5432/testdb");
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_pool_options() {
        let config = PostgresConfig::new("postgres://user:pass@localhost:5432/testdb")
            .with_max_connections(15)
            .with_min_connections(3);

        let _opts = pool_options(&config);
    }

    #[tokio::test]
    async fn test_pool_creation() {
        let config = PostgresConfig::new("postgres://user:pass@localhost:5432/testdb")
            .with_max_connections(5);

        let pool = create_pool(&config);
        assert!(pool.is_ok());
    }
}
