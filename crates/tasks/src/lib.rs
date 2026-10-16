//! Typed, explicitly named Apalis queues. No application lifecycle or domain handlers.
pub mod board;
pub mod config;
pub mod worker;
pub use apalis;
pub use apalis::prelude::{Status, Task, TaskId};
use apalis::prelude::{TaskBuilder, WorkerBuilder, WorkerBuilderExt};
pub use apalis_postgres::{self, PgTask, PgTaskId, PostgresStorage};
pub use config::WorkerConfig;
use errors::TasksError;
use sqlx::PgPool;

pub struct Queue<Args> {
    storage: PostgresStorage<Args>,
    config: WorkerConfig,
}
impl<Args> Queue<Args> {
    pub fn new(pool: PgPool, name: &str, config: WorkerConfig) -> Result<Self, TasksError> {
        config.validate()?;
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(TasksError::Config("Queue name must contain 1..128 ASCII letters, digits, dots, underscores or hyphens".into()));
        }
        let storage_config = apalis_postgres::Config::default()
            .queue(name)
            .batch_size(config.concurrency)
            .heartbeat_interval(config.heartbeat_interval)
            .missed_heartbeats(config.missed_heartbeats);
        Ok(Self {
            storage: PostgresStorage::new(&pool).with_config(storage_config),
            config,
        })
    }
    pub fn storage(&self) -> PostgresStorage<Args> {
        self.storage.clone()
    }
    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }
    /// Upstream task, suitable for TaskSink or transactional queries::push_tasks.
    pub fn task(&self, args: Args) -> Task<Args> {
        TaskBuilder::new(args)
            .task_id(TaskId::from_ulid(ulid::Ulid::generate()))
            .max_attempts(self.config.max_attempts)
            .build()
    }
    /// Check worker readiness and register a typed handler with build().
    /// Run it with lifecycle.run_until so cancellation retires its pool safely.
    pub async fn worker(
        &self,
        lifecycle: &worker::Lifecycle,
    ) -> Result<worker::Builder<Args>, TasksError> {
        check_ready(lifecycle.pool()).await?;
        Ok(WorkerBuilder::new(lifecycle.context())
            .backend(
                PostgresStorage::new(lifecycle.pool()).with_config(self.storage.config().clone()),
            )
            .concurrency(self.config.concurrency)
            .retry(worker::policy(&self.config)))
    }
    pub async fn check_ready(&self) -> Result<(), TasksError> {
        check_ready(self.storage.pool()).await
    }
}

/// Runtime readiness uses a narrow owner-defined contract, never ledger access or DDL.
pub async fn check_ready(pool: &PgPool) -> Result<(), TasksError> {
    let user: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(pool)
        .await
        .map_err(|_| TasksError::NotReady)?;
    if user != "hypogeum_tasks" {
        return Err(TasksError::NotReady);
    }
    let version: i64 = sqlx::query_scalar("SELECT apalis.schema_version()")
        .fetch_one(pool)
        .await
        .map_err(|_| TasksError::NotReady)?;
    if version != 20260902075949 {
        return Err(TasksError::NotReady);
    }
    Ok(())
}
