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
        }
    }
}
impl WorkerConfig {
    pub fn validate(&self) -> Result<(), TasksError> {
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
