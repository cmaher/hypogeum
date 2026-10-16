use crate::WorkerConfig;
use apalis::prelude::*;
use errors::TasksError;
use std::{future::Future, time::Duration};
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

/// Stop claiming on signal, drain and flush upstream persistence, then drop the
/// worker future at the deadline. Abandoned claims are recovered by Apalis.
pub async fn run_until<F, S>(
    context: &WorkerContext,
    worker: F,
    signal: S,
    timeout: Duration,
) -> Result<(), TasksError>
where
    F: Future<Output = Result<(), WorkerError>>,
    S: Future<Output = ()>,
{
    tokio::pin!(worker);
    tokio::pin!(signal);
    tokio::select! {
        biased;
        result = &mut worker => result.map_err(TasksError::operation),
        () = &mut signal => {
            context.stop().map_err(TasksError::operation)?;
            tokio::time::timeout(timeout, &mut worker).await
                .map_err(|_| TasksError::DrainTimeout)?.map_err(TasksError::operation)
        }
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
