//! Postgres connection pooling utilities.
use crate::pool_config::PoolConfig;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub fn pool_options(config: &PoolConfig) -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.connect_timeout)
        .idle_timeout(Some(config.idle_timeout))
}

pub fn create_pool(config: &PoolConfig) -> Result<PgPool, sqlx::Error> {
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
    fn test_pool_options() {
        let config = PoolConfig::new("postgres://user:pass@localhost:5432/testdb")
            .with_max_connections(15)
            .with_min_connections(3)
            .with_connect_timeout(std::time::Duration::from_secs(8))
            .with_idle_timeout(std::time::Duration::from_secs(120));

        let opts = pool_options(&config);
        let debug = format!("{opts:?}");
        assert!(debug.contains("max_connections: 15"));
        assert!(debug.contains("min_connections: 3"));
        assert!(debug.contains("connect_timeout: 8s"));
        assert!(debug.contains("idle_timeout: Some(120s)"));
    }

    #[tokio::test]
    async fn test_pool_creation() {
        let config =
            PoolConfig::new("postgres://user:pass@localhost:5432/testdb").with_max_connections(5);

        let pool = create_pool(&config);
        assert!(pool.is_ok());
    }
}
