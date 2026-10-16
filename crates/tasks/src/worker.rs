use crate::WorkerConfig;
use apalis::prelude::*;
use errors::TasksError;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tower::{
    layer::util::{Identity, Stack},
    limit::ConcurrencyLimitLayer,
    retry::{
        RetryLayer,
        backoff::{Backoff, ExponentialBackoff, ExponentialBackoffMaker, MakeBackoff},
    },
};

/// Tower retry policy using upstream Apalis attempts, status and error semantics.
#[derive(Debug, Clone)]
pub struct Policy {
    max_attempts: usize,
    backoff: ExponentialBackoff,
}
pub type Middleware = Stack<RetryLayer<Policy>, Stack<ConcurrencyLimitLayer, Identity>>;
pub type Builder<Args> = WorkerBuilder<Args, apalis_postgres::PostgresStorage<Args>, Middleware>;

pub(crate) fn policy(config: &WorkerConfig) -> Policy {
    let backoff = ExponentialBackoffMaker::new(
        config.backoff_initial,
        config.backoff_max,
        0.0,
        apalis::layers::retry::HasherRng::default(),
    )
    .expect("validated backoff bounds")
    .make_backoff();
    Policy {
        max_attempts: config.max_attempts,
        backoff,
    }
}

impl<Args: Clone, Response> tower::retry::Policy<Task<Args>, Response, BoxDynError> for Policy {
    type Future = tokio::time::Sleep;

    fn retry(
        &mut self,
        task: &mut Task<Args>,
        result: &mut Result<Response, BoxDynError>,
    ) -> Option<Self::Future> {
        let error = result.as_ref().err()?;
        if task.status() == Status::Killed || error.downcast_ref::<AbortError>().is_some() {
            return None;
        }
        let limit = task
            .max_attempts()
            .unwrap_or(self.max_attempts)
            .min(self.max_attempts);
        if task.attempt() >= limit {
            // Pending retry-after/deferred errors must become terminal too.
            // A raw upstream submission can have a larger persisted budget;
            // Killed prevents the backend from claiming it beyond our cap.
            let status = if task.max_attempts().is_some_and(|stored| stored > limit) {
                Status::Killed
            } else {
                Status::Failed
            };
            task.raw_status().store(status);
            return None;
        }
        if task.data().get::<WorkerContext>()?.is_shutting_down() {
            task.raw_status().store(Status::Failed);
            return None;
        }
        if let Some(error) = error.downcast_ref::<RetryAfterError>() {
            return Some(tokio::time::sleep(error.get_duration()));
        }
        if error.downcast_ref::<DeferredError>().is_some() {
            return Some(tokio::time::sleep(Duration::ZERO));
        }
        Some(self.backoff.next_backoff())
    }

    fn clone_request(&mut self, task: &Task<Args>) -> Option<Task<Args>> {
        Some(task.clone())
    }
}

/// A single worker's pool and shutdown state. Never share this pool with producers
/// or unrelated workers. Construct a fresh lifecycle for every worker restart.
pub struct Lifecycle {
    context: WorkerContext,
    pool: PgPool,
    retired: Arc<AtomicBool>,
    closed: bool,
}

impl Lifecycle {
    /// Pool sizing is explicit: budget these connections separately from the
    /// application pool. The after_release callback is reserved for shutdown.
    pub fn new(name: &str, options: PgPoolOptions, connection: PgConnectOptions) -> Self {
        let context = WorkerContext::new(name);
        let retired = Arc::new(AtomicBool::new(false));
        let state = retired.clone();
        let worker = context.clone();
        let pool = options
            .after_release(move |_, _| {
                // No SQL or round trip on connection return. Once stopping, discard
                // physical sessions: SQLx 0.9 can miss rollback if BEGIN is cancelled.
                if worker.is_shutting_down() {
                    state.store(true, Ordering::SeqCst);
                }
                let reuse = !state.load(Ordering::SeqCst);
                Box::pin(async move { Ok(reuse) })
            })
            .connect_lazy_with(connection);
        Self {
            context,
            pool,
            retired,
            closed: false,
        }
    }

    pub fn context(&self) -> &WorkerContext {
        &self.context
    }

    /// Worker infrastructure only; producers and application queries need their
    /// own pool. This pool is permanently closed when the lifecycle ends.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn close(&mut self) {
        self.pool.close().await;
        self.closed = true;
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
        if !self.context.is_shutting_down() {
            let _ = self.context.stop();
        }
    }

    /// Stop claiming on signal and share one deadline between draining the
    /// worker and closing its pool. Worker completion/errors without a signal
    /// start a cleanup-only deadline. Expiry returns DrainTimeout while final
    /// cleanup continues on Tokio. Cancellation also retires the pool before
    /// dropping the worker, including cancellation before the first poll.
    pub fn run_until<F, S>(
        self,
        worker: F,
        signal: S,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), TasksError>>
    where
        F: Future<Output = Result<(), WorkerError>>,
        S: Future<Output = ()>,
    {
        // Construct synchronously so even an unpolled future owns the drop guard.
        let mut running = Running {
            lifecycle: self,
            worker: Some(Box::pin(worker)),
        };
        async move {
            let (result, deadline) = {
                let worker = running
                    .worker
                    .as_mut()
                    .expect("worker present until drain ends");
                tokio::pin!(signal);
                tokio::select! {
                    biased;
                    result = &mut *worker => (
                        result.map_err(TasksError::operation),
                        tokio::time::Instant::now() + timeout,
                    ),
                    () = &mut signal => {
                        let deadline = tokio::time::Instant::now() + timeout;
                        running.lifecycle.retire();
                        let result = tokio::time::timeout_at(deadline, worker).await
                            .map_err(|_| TasksError::DrainTimeout)
                            .and_then(|result| result.map_err(TasksError::operation));
                        (result, deadline)
                    }
                }
            };
            running.lifecycle.retire();
            drop(running.worker.take());
            // Mark closure synchronously, even if no cleanup budget remains.
            // On expiry the drop guard finishes physical cleanup in the background.
            drop(running.lifecycle.pool.close());
            tokio::time::timeout_at(deadline, running.lifecycle.close())
                .await
                .map_err(|_| TasksError::DrainTimeout)?;
            result
        }
    }
}

impl Drop for Lifecycle {
    fn drop(&mut self) {
        self.retire();
        if !self.closed {
            // close() marks the pool closed synchronously; the task finishes
            // physical session cleanup when the outer future was cancelled.
            drop(self.pool.close());
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let pool = self.pool.clone();
                runtime.spawn(async move { pool.close().await });
            }
        }
    }
}

struct Running<F> {
    lifecycle: Lifecycle,
    worker: Option<Pin<Box<F>>>,
}

impl<F> Drop for Running<F> {
    fn drop(&mut self) {
        self.lifecycle.retire();
        drop(self.worker.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::retry::Policy as _;

    fn request(attempt: usize, max_attempts: usize, status: Status) -> Task<u32> {
        let mut context = WorkerContext::new("retry-policy-fixture");
        context.start().unwrap();
        TaskBuilder::new(42)
            .attempt(attempt)
            .max_attempts(max_attempts)
            .status(status)
            .data(context)
            .build()
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_honors_requested_delay() {
        let mut policy = policy(&WorkerConfig::default());
        let mut task = request(1, 5, Status::Pending);
        let delay = Duration::from_secs(60);
        let mut result: Result<(), BoxDynError> =
            Err(RetryAfterError::new("temporary", delay).into());
        let started = tokio::time::Instant::now();
        policy
            .retry(&mut task, &mut result)
            .expect("retry-after must schedule a retry")
            .await;
        assert_eq!(started.elapsed(), delay);
    }

    #[tokio::test]
    async fn retry_after_exhaustion_is_persisted_as_failed() {
        let mut policy = policy(&WorkerConfig::default());
        let mut task = request(2, 2, Status::Pending);
        let mut result: Result<(), BoxDynError> =
            Err(RetryAfterError::new("temporary", Duration::from_secs(60)).into());
        assert!(policy.retry(&mut task, &mut result).is_none());
        assert_eq!(task.status(), Status::Failed);
    }

    #[tokio::test(start_paused = true)]
    async fn deferral_retries_immediately_with_a_bounded_attempt_budget() {
        let mut policy = policy(&WorkerConfig::default());
        let mut task = request(1, 2, Status::Pending);
        let mut result: Result<(), BoxDynError> = Err(DeferredError::new("deferred").into());
        policy
            .retry(&mut task, &mut result)
            .expect("deferral must retry")
            .await;
        let mut last = request(2, 2, Status::Pending);
        assert!(policy.retry(&mut last, &mut result).is_none());
        assert_eq!(last.status(), Status::Failed);
    }

    #[tokio::test]
    async fn ordinary_errors_respect_the_lower_task_limit() {
        let mut policy = policy(&WorkerConfig::default());
        let mut task = request(2, 2, Status::Failed);
        let mut result: Result<(), BoxDynError> = Err("temporary".into());
        assert!(policy.retry(&mut task, &mut result).is_none());
    }

    #[tokio::test]
    async fn abort_errors_are_never_retried() {
        let mut policy = policy(&WorkerConfig::default());
        let mut task = request(1, 5, Status::Killed);
        let mut result: Result<(), BoxDynError> = Err(AbortError::new("permanent").into());
        assert!(policy.retry(&mut task, &mut result).is_none());
        assert_eq!(task.status(), Status::Killed);
    }
}
