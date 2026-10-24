//! Relational database pooling, service contracts, and application-driven migrations.

pub mod contract;
pub mod pool_config;

pub use contract::{
    DatabaseConfig, DomainMigrator, SeaOrmAdapter, ServiceContract, validate_identifier,
};
pub use pool_config::PoolConfig;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "postgres")]
pub use postgres::{PgPool, create_pool, inspect, migrate, ping, pool_options, verify};

// Backwards compatibility alias
pub type PostgresConfig = PoolConfig;
