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
            .with_min_connections(3);

        let _opts = pool_options(&config);
    }

    #[tokio::test]
    async fn test_pool_creation() {
        let config = PoolConfig::new("postgres://user:pass@localhost:5432/testdb")
            .with_max_connections(5);

        let pool = create_pool(&config);
        assert!(pool.is_ok());
    }
}
