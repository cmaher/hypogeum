use hypgm_errors::TasksError;
use std::time::Duration;

/// Explicit infrastructure policy supplied by the composition root.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub concurrency: usize,
    pub max_attempts: usize,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub shutdown_timeout: Duration,
    pub heartbeat_interval: Duration,
    pub missed_heartbeats: usize,
    pub tasks_role: String,
}
impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: 1,
            max_attempts: 5,
            backoff_initial: Duration::from_secs(5),
            backoff_max: Duration::from_secs(300),
            shutdown_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(30),
            missed_heartbeats: 2,
            tasks_role: "hypogeum_tasks".to_string(),
        }
    }
}
impl WorkerConfig {
    pub fn with_tasks_role(mut self, tasks_role: impl Into<String>) -> Self {
        self.tasks_role = tasks_role.into();
        self
    }

    pub fn validate(&self) -> Result<(), TasksError> {
        hypgmx_rdbms::validate_identifier(&self.tasks_role)
            .map_err(|e| TasksError::Config(e.to_string()))?;
        // Apalis converts the orphan window to whole seconds, then PostgreSQL
        // interval microseconds. Validate both conversions before building storage.
        let orphan_window = u32::try_from(self.missed_heartbeats)
            .ok()
            .and_then(|missed| self.heartbeat_interval.checked_mul(missed));
        if self.concurrency == 0
            || self.concurrency > i32::MAX as usize
            || self.max_attempts == 0
            || self.max_attempts > i32::MAX as usize
            || self.backoff_initial.is_zero()
            || self.backoff_max < self.backoff_initial
            || self.shutdown_timeout.is_zero()
            || self.heartbeat_interval.is_zero()
            || self.missed_heartbeats == 0
            || orphan_window.is_none_or(|window| {
                window.as_secs() == 0 || window.as_secs() > i64::MAX as u64 / 1_000_000
            })
        {
            return Err(TasksError::Config(
                "Invalid concurrency, attempts or duration bounds".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_config_role_validation() {
        let valid = WorkerConfig::default();
        assert!(valid.validate().is_ok());

        let custom = WorkerConfig::default().with_tasks_role("exemplum_tasks");
        assert!(custom.validate().is_ok());
        assert_eq!(custom.tasks_role, "exemplum_tasks");

        let invalid_empty = WorkerConfig {
            tasks_role: "".into(),
            ..Default::default()
        };
        assert!(invalid_empty.validate().is_err());

        let invalid_chars = WorkerConfig {
            tasks_role: "invalid-role!".into(),
            ..Default::default()
        };
        assert!(invalid_chars.validate().is_err());
    }
}
