use apalis::prelude::*;
use sqlx::PgPool;
use std::time::Duration;
use tasks::{Queue, WorkerConfig};

#[tokio::test]
async fn named_queue_and_task_use_upstream_configuration() {
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let queue = Queue::<u32>::new(pool.clone(), "fixture", WorkerConfig::default()).unwrap();
    let task = queue.task(42);
    assert_eq!(queue.storage().config().queue.as_ref(), "fixture");
    assert_eq!(task.max_attempts(), Some(5));
    assert!(task.task_id().is_some());
    assert!(Queue::<u32>::new(pool.clone(), "", WorkerConfig::default()).is_err());
    assert!(
        Queue::<u32>::new(
            pool,
            "fixture",
            WorkerConfig {
                concurrency: 0,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn shutdown_stops_claiming_and_bounds_drain() {
    let context = WorkerContext::new("fixture-shutdown");
    let mut starter = context.clone();
    starter.start().unwrap();
    let result = tasks::worker::run_until(
        &context,
        std::future::pending(),
        async {},
        Duration::from_millis(10),
    )
    .await;
    assert!(context.is_stopped());
    assert!(result.unwrap_err().to_string().contains("drain"));
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Effect {
    key: String,
    fail_until: i32,
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn typed_handler_executes_retries_and_exhausts(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let pool = PgPool::connect(&db.tasks_url).await.unwrap();
    let admin = PgPool::connect(&db.admin_url).await.unwrap();
    sqlx::raw_sql("CREATE SCHEMA task_fixture; CREATE TABLE task_fixture.effects(key text PRIMARY KEY, attempts integer NOT NULL); GRANT USAGE ON SCHEMA task_fixture TO foundation_tasks; GRANT SELECT,INSERT,UPDATE ON task_fixture.effects TO foundation_tasks;").execute(&admin).await.unwrap();
    for (suffix, fail_until, expected, status) in [
        ("retry", 2, 3, Status::Done),
        ("exhaust", 10, 5, Status::Failed),
        ("delay", 2, 3, Status::Done),
        ("delay-exhaust", 10, 5, Status::Failed),
        ("defer-exhaust", 10, 5, Status::Failed),
        ("raw-exhaust", 10, 5, Status::Killed),
    ] {
        let config = WorkerConfig {
            backoff_initial: Duration::from_millis(10),
            backoff_max: Duration::from_millis(40),
            ..Default::default()
        };
        let queue = Queue::new(pool.clone(), suffix, config.clone()).unwrap();
        queue.check_ready().await.unwrap();
        let effect = Effect {
            key: suffix.into(),
            fail_until,
        };
        let task = if suffix == "raw-exhaust" {
            TaskBuilder::new(effect)
                .task_id(TaskId::from_ulid(ulid::Ulid::generate()))
                .max_attempts(25)
                .build()
        } else {
            queue.task(effect)
        };
        let id = task.task_id().unwrap().clone();
        queue.storage().push_task(task).await.unwrap();
        let context = WorkerContext::new(suffix);
        let effect_pool = pool.clone();
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let times = attempts.clone();
        let worker = queue
            .worker(&context)
            .await
            .unwrap()
            .build(move |effect: Effect| {
                let pool = effect_pool.clone();
                times.lock().unwrap().push(std::time::Instant::now());
                execute_effect(effect, pool, suffix)
            });
        let completed = wait_completed(&queue, &id, status, expected as usize);
        tasks::worker::run_until(&context, worker.run(), completed, config.shutdown_timeout)
            .await
            .unwrap();
        let actual: i32 =
            sqlx::query_scalar("SELECT attempts FROM task_fixture.effects WHERE key=$1")
                .bind(suffix)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(actual, expected);
        if suffix.starts_with("delay") {
            assert!(
                attempts
                    .lock()
                    .unwrap()
                    .windows(2)
                    .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(200))
            );
        }
    }
    pool.close().await;
    admin.close().await;
}

#[test]
fn liveness_windows_must_survive_postgres_seconds_conversion() {
    for (heartbeat, missed) in [(Duration::from_millis(100), 2), (Duration::MAX, 2)] {
        let config = WorkerConfig {
            heartbeat_interval: heartbeat,
            missed_heartbeats: missed,
            ..Default::default()
        };
        assert!(
            config.validate().is_err(),
            "accepted unrepresentable orphan window: {config:?}"
        );
    }
    assert!(
        WorkerConfig {
            heartbeat_interval: Duration::from_millis(500),
            missed_heartbeats: 2,
            ..Default::default()
        }
        .validate()
        .is_ok()
    );
}

async fn execute_effect(effect: Effect, pool: PgPool, suffix: &str) -> Result<(), BoxDynError> {
    let count: i32 = sqlx::query_scalar("INSERT INTO task_fixture.effects VALUES($1,1) ON CONFLICT(key) DO UPDATE SET attempts=task_fixture.effects.attempts+1 RETURNING attempts").bind(effect.key).fetch_one(&pool).await?;
    if count > effect.fail_until {
        return Ok(());
    }
    if suffix.starts_with("delay") {
        return Err(RetryAfterError::new("temporary", Duration::from_millis(200)).into());
    }
    if suffix.starts_with("defer") {
        return Err(DeferredError::new("temporary").into());
    }
    Err("fixture failure".into())
}

async fn wait_completed(queue: &Queue<Effect>, id: &TaskId, status: Status, expected: usize) {
    let mut storage = queue.storage();
    let completed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if storage
                .fetch_by_id(id)
                .await
                .unwrap()
                .is_some_and(|task| task.status() == status && task.attempt() == expected)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if completed.is_err() {
        let actual = storage
            .fetch_by_id(id)
            .await
            .unwrap()
            .map(|task| (task.status(), task.attempt(), task.max_attempts()));
        panic!(
            "queue {} expected {status:?} after {expected} attempts, got {actual:?}",
            queue.storage().config().queue
        );
    }
}

// Poll BEGIN only until it is sent, then shut down before its reply is read.
// This makes the cancellation window deterministic instead of relying on timing
// between the worker's next fetch and the completion signal.
#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn shutdown_finishes_pending_fetch_without_leaking_transactions(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use std::{future::poll_fn, task::Poll};

    let db = test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect(&db.tasks_url)
        .await
        .unwrap();
    let admin = PgPool::connect(&db.admin_url).await.unwrap();

    for submit in [false, true] {
        let queue =
            Queue::<u32>::new(pool.clone(), "shutdown-fetch", WorkerConfig::default()).unwrap();
        let mut storage = queue.storage();
        let task = queue.task(42);
        let id = task.task_id().unwrap().clone();
        if submit {
            storage.push_task(task).await.unwrap();
        }
        let mut context = WorkerContext::new("shutdown-fetch");
        context.start().unwrap();
        poll_fn(|cx| storage.poll_ready(cx, &context))
            .await
            .unwrap();
        // Pool return is asynchronous. An idle connection lets the next poll
        // reach BEGIN immediately, without suspending on acquisition or ping.
        tokio::time::timeout(Duration::from_secs(5), async {
            while pool.num_idle() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(poll_fn(|cx| Poll::Ready(storage.poll_next(cx, &context).is_pending())).await);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let begun: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND usename='foundation_tasks' AND state='idle in transaction' AND query='BEGIN')").fetch_one(&admin).await.unwrap();
                if begun { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();

        context.stop().unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            poll_fn(|cx| storage.poll_close(cx, &context)),
        )
        .await
        .unwrap()
        .unwrap();
        // Reacquiring waits for SQLx's return-to-pool cleanup to finish.
        let connection = pool.acquire().await.unwrap();
        let leaked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='foundation_tasks' AND state='idle in transaction'").fetch_one(&admin).await.unwrap();
        assert_eq!(
            leaked, 0,
            "shutdown returned a connection with an open transaction"
        );
        drop(connection);
        if submit {
            let task = storage.fetch_by_id(&id).await.unwrap().unwrap();
            assert_eq!(
                task.status(),
                Status::Pending,
                "shutdown must release fetched claims"
            );
        }
    }
    pool.close().await;
    admin.close().await;
}
