//! Generic relational database connection pool configuration.
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
}

impl std::fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("url", &"[redacted]")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout", &self.connect_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .finish()
    }
}

impl PoolConfig {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_construction() {
        let config = PoolConfig::new("postgres://user:pass@localhost:5432/testdb")
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
    fn test_pool_config_debug_redacts_url() {
        let config = PoolConfig::new("postgres://user:super_secret_password@localhost:5432/testdb");
        let debug_str = format!("{config:?}");
        assert!(!debug_str.contains("super_secret_password"));
        assert!(!debug_str.contains("postgres://"));
        assert!(debug_str.contains("[redacted]"));
        assert!(debug_str.contains("max_connections"));
        assert!(debug_str.contains("min_connections"));
        assert!(debug_str.contains("connect_timeout"));
        assert!(debug_str.contains("idle_timeout"));
    }
}
