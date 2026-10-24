//! Durable welcome work owned by the users domain.
use hypgmx_tasks::{Queue, WorkerConfig};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const WELCOME_QUEUE: &str = "exemplum.welcome.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WelcomeJob {
    pub user_id: Uuid,
}

/// Leave the operational retry budget to the consumer. PostgreSQL requires a
/// persisted integer limit, so use its storage bound rather than a second policy.
pub fn welcome_task(user_id: Uuid) -> hypgmx_tasks::Task<WelcomeJob> {
    hypgmx_tasks::apalis::prelude::TaskBuilder::new(WelcomeJob { user_id })
        .max_attempts(i32::MAX as usize)
        .build()
}

pub fn welcome_queue(
    pool: sqlx::PgPool,
    config: WorkerConfig,
) -> Result<Queue<WelcomeJob>, hypgm_errors::TasksError> {
    Queue::new(pool, WELCOME_QUEUE, config)
}

/// Inject a users-role repository, independently of the queue lifecycle pool.
pub async fn handle_welcome(
    job: WelcomeJob,
    repo: std::sync::Arc<dyn crate::accounts::WelcomeRepo>,
) -> Result<(), crate::accounts::AccountError> {
    repo.process_welcome(job.user_id).await
}
