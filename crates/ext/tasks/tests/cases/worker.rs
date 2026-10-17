use apalis::prelude::*;
use hypgmx_tasks::{Queue, WorkerConfig};
use sqlx::PgPool;
use std::time::Duration;

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
    let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
        "fixture-shutdown",
        sqlx::postgres::PgPoolOptions::new(),
        "postgres://unused:unused@localhost/unused".parse().unwrap(),
    );
    let context = lifecycle.context().clone();
    let mut starter = context.clone();
    starter.start().unwrap();
    let result = lifecycle
        .run_until(std::future::pending(), async {}, Duration::from_millis(10))
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
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let pool = PgPool::connect(&db.tasks_url).await.unwrap();
    let admin = PgPool::connect(&db.admin_url).await.unwrap();
    sqlx::raw_sql("CREATE SCHEMA task_fixture; CREATE TABLE task_fixture.effects(key text PRIMARY KEY, attempts integer NOT NULL); GRANT USAGE ON SCHEMA task_fixture TO hypogeum_tasks; GRANT SELECT,INSERT,UPDATE ON task_fixture.effects TO hypogeum_tasks;").execute(&admin).await.unwrap();
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
        let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
            suffix,
            sqlx::postgres::PgPoolOptions::new().max_connections(2),
            db.tasks_url.parse().unwrap(),
        );
        let effect_pool = pool.clone();
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let times = attempts.clone();
        let worker = queue
            .worker(&lifecycle)
            .await
            .unwrap()
            .build(move |effect: Effect| {
                let pool = effect_pool.clone();
                times.lock().unwrap().push(std::time::Instant::now());
                execute_effect(effect, pool, suffix)
            });
        let completed = wait_completed(&queue, &id, status, expected as usize);
        lifecycle
            .run_until(worker.run(), completed, config.shutdown_timeout)
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

// Hold BEGIN's server reply to make SQLx's cancellation window deterministic.
#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn shutdown_retires_pending_fetch_connections(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let producer = PgPool::connect(&db.tasks_url).await.unwrap();
    let admin = PgPool::connect(&db.admin_url).await.unwrap();
    for submit in [false, true] {
        for mode in ["signal", "cancel", "self-stop"] {
            let options: sqlx::postgres::PgConnectOptions = db.tasks_url.parse().unwrap();
            let mut barrier =
                super::begin_barrier::BeginBarrier::start(options.get_host(), options.get_port())
                    .await;
            let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
                "shutdown-fetch",
                sqlx::postgres::PgPoolOptions::new().max_connections(1),
                options
                    .host("127.0.0.1")
                    .port(barrier.port())
                    .ssl_mode(sqlx::postgres::PgSslMode::Disable),
            );
            let worker_pool = lifecycle.pool().clone();
            let context = lifecycle.context().clone();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&worker_pool)
                .await
                .unwrap();
            // Normal transactions and returns retain the same physical session.
            let tx = worker_pool.begin().await.unwrap();
            tx.commit().await.unwrap();
            let reused: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&worker_pool)
                .await
                .unwrap();
            assert_eq!(pid, reused);
            let queue =
                Queue::<u32>::new(producer.clone(), "shutdown-fetch", WorkerConfig::default())
                    .unwrap();
            let task = queue.task(42);
            let id = task.task_id().unwrap().clone();
            if submit {
                queue.storage().push_task(task).await.unwrap();
            }
            let armed = barrier.arm_handle();
            let worker = queue
                .worker(&lifecycle)
                .await
                .unwrap()
                .on_event(move |_, event| {
                    // Registration also uses BEGIN. Arm only after it commits.
                    if matches!(event, Event::HeartBeat) {
                        armed.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                })
                .build(|_: u32| async { Ok::<_, BoxDynError>(()) });
            let (stop, signal) = tokio::sync::oneshot::channel();
            let future = lifecycle.run_until(
                worker.run(),
                async {
                    let _ = signal.await;
                },
                Duration::from_secs(5),
            );
            let mut future = Box::pin(future);
            let mut exhaust_budget = true;
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    biased;
                    () = barrier.reached() => {},
                    result = std::future::poll_fn(|cx| {
                        use std::future::Future;
                        if exhaust_budget {
                            exhaust_budget = false;
                            while let std::task::Poll::Ready(guard) = tokio::task::coop::poll_proceed(cx) {
                                guard.made_progress();
                            }
                        }
                        // An early Pending before BEGIN must not strand the test.
                        future.as_mut().poll(cx)
                    }) => panic!("worker passed BEGIN barrier: {result:?}"),
                }
            })
            .await
            .unwrap();
            match mode {
                "cancel" => {
                    drop(future);
                    barrier.release();
                }
                "self-stop" => {
                    context.stop().unwrap();
                    barrier.release();
                    future.await.unwrap();
                }
                _ => {
                    stop.send(()).unwrap();
                    barrier.release();
                    future.await.unwrap();
                }
            }
            assert!(worker_pool.is_closed());
            tokio::time::timeout(Duration::from_secs(5), worker_pool.close())
                .await
                .unwrap();
            // SQLx closes the client socket before the proxy and PostgreSQL
            // necessarily process its disconnect. Await server-side removal.
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let alive: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT FROM pg_stat_activity WHERE pid=$1)",
                    )
                    .bind(pid)
                    .fetch_one(&admin)
                    .await
                    .unwrap();
                    if !alive {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("cancelled BEGIN session survived shutdown ({mode})"));
            if submit {
                let task = queue.storage().fetch_by_id(&id).await.unwrap().unwrap();
                assert_eq!(task.status(), Status::Pending);
            }
            tokio::time::timeout(Duration::from_secs(5), barrier.finish())
                .await
                .unwrap();
        }
    }
    // Unrelated producers stay usable after every worker lifecycle has ended.
    producer.begin().await.unwrap().commit().await.unwrap();
    producer.close().await;
    admin.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn shutdown_drains_acknowledgments_and_recovers_forced_claims(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use std::sync::Arc;
    use tokio::sync::Notify;
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let producer = PgPool::connect(&db.tasks_url).await.unwrap();
    let config = WorkerConfig {
        heartbeat_interval: Duration::from_secs(1),
        missed_heartbeats: 1,
        ..Default::default()
    };
    for mode in ["drain", "deadline", "cancel"] {
        let queue = Queue::<Effect>::new(producer.clone(), mode, config.clone()).unwrap();
        let task = queue.task(Effect {
            key: mode.into(),
            fail_until: 0,
        });
        let id = task.task_id().unwrap().clone();
        queue.storage().push_task(task).await.unwrap();
        let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
            mode,
            sqlx::postgres::PgPoolOptions::new().max_connections(2),
            db.tasks_url.parse().unwrap(),
        );
        let worker_pool = lifecycle.pool().clone();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_release = release.clone();
        let worker = queue
            .worker(&lifecycle)
            .await
            .unwrap()
            .build(move |_: Effect| {
                let started = handler_started.clone();
                let release = handler_release.clone();
                async move {
                    started.notify_one();
                    release.notified().await;
                    Ok::<_, BoxDynError>(())
                }
            });
        let future = lifecycle.run_until(
            worker.run(),
            async {
                if mode == "cancel" {
                    std::future::pending::<()>().await;
                }
                started.notified().await;
                if mode == "drain" {
                    release.notify_one();
                }
            },
            if mode == "deadline" {
                Duration::from_millis(20)
            } else {
                config.shutdown_timeout
            },
        );
        if mode == "cancel" {
            let mut future = Box::pin(future);
            tokio::select! {
                biased;
                () = started.notified() => {},
                result = &mut future => panic!("worker exited before handler started: {result:?}"),
            }
            drop(future);
        } else {
            let result = future.await;
            if mode == "deadline" {
                assert!(matches!(
                    result,
                    Err(hypgm_errors::TasksError::DrainTimeout)
                ));
            } else {
                result.unwrap();
            }
        }
        assert!(worker_pool.is_closed());
        tokio::time::timeout(Duration::from_secs(5), worker_pool.close())
            .await
            .unwrap();
        let persisted = queue.storage().fetch_by_id(&id).await.unwrap().unwrap();
        if mode == "drain" {
            assert_eq!(
                persisted.status(),
                Status::Done,
                "ack must flush before closing the pool"
            );
        } else {
            assert_ne!(persisted.status(), Status::Done);
            let replacement = hypgmx_tasks::worker::Lifecycle::new(
                &format!("{mode}-replacement"),
                sqlx::postgres::PgPoolOptions::new().max_connections(2),
                db.tasks_url.parse().unwrap(),
            );
            let worker = queue
                .worker(&replacement)
                .await
                .unwrap()
                .build(|_: Effect| async { Ok::<_, BoxDynError>(()) });
            replacement
                .run_until(
                    worker.run(),
                    wait_completed(&queue, &id, Status::Done, 2),
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
        }
    }
    producer.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn shutdown_requeues_committed_buffered_claims(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use std::{future::poll_fn, sync::Arc};
    use tokio::sync::Notify;
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let producer = PgPool::connect(&db.tasks_url).await.unwrap();
    let config = WorkerConfig {
        concurrency: 3,
        ..Default::default()
    };
    let queue = Queue::<u32>::new(producer.clone(), "buffered", config.clone()).unwrap();
    let mut ids = Vec::new();
    for value in 0..3 {
        let task = queue.task(value);
        ids.push(task.task_id().unwrap().clone());
        queue.storage().push_task(task).await.unwrap();
    }
    let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
        "buffered",
        sqlx::postgres::PgPoolOptions::new().max_connections(1),
        db.tasks_url.parse().unwrap(),
    );
    let context = lifecycle.context().clone();
    let worker_queue = Queue::<u32>::new(lifecycle.pool().clone(), "buffered", config).unwrap();
    let fetched = Arc::new(Notify::new());
    let close = Arc::new(Notify::new());
    let worker_fetched = fetched.clone();
    let worker_close = close.clone();
    let worker = async move {
        let mut context = context;
        context.start()?;
        let mut storage = worker_queue.storage();
        poll_fn(|cx| storage.poll_ready(cx, &context))
            .await
            .map_err(|e| WorkerError::BackendError(e.into()))?;
        let _first = poll_fn(|cx| storage.poll_next(cx, &context))
            .await
            .unwrap()
            .unwrap();
        worker_fetched.notify_one();
        worker_close.notified().await;
        poll_fn(|cx| storage.poll_close(cx, &context))
            .await
            .map_err(|e| WorkerError::BackendError(e.into()))?;
        Ok(())
    };
    lifecycle
        .run_until(
            worker,
            async {
                fetched.notified().await;
                close.notify_one();
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let mut running = 0;
    let mut pending = 0;
    for id in ids {
        match queue
            .storage()
            .fetch_by_id(&id)
            .await
            .unwrap()
            .unwrap()
            .status()
        {
            Status::Queued => running += 1,
            Status::Pending => pending += 1,
            other => panic!("unexpected buffered task status: {other:?}"),
        }
    }
    assert_eq!(
        (running, pending),
        (1, 2),
        "unconsumed claims must be requeued"
    );
    producer.close().await;
}

#[tokio::test]
async fn unpolled_and_error_workers_retire_the_pool_before_drop() {
    struct CheckDrop {
        context: WorkerContext,
    }
    impl Drop for CheckDrop {
        fn drop(&mut self) {
            assert!(self.context.is_shutting_down());
        }
    }
    impl std::future::Future for CheckDrop {
        type Output = Result<(), WorkerError>;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Ready(Err(WorkerError::BackendError("fixture failure".into())))
        }
    }
    for poll in [false, true] {
        let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
            "drop-order",
            sqlx::postgres::PgPoolOptions::new(),
            "postgres://unused:unused@localhost/unused".parse().unwrap(),
        );
        let mut context = lifecycle.context().clone();
        context.start().unwrap();
        let pool = lifecycle.pool().clone();
        let worker = CheckDrop { context };
        let future = lifecycle.run_until(worker, std::future::pending(), Duration::from_secs(1));
        if poll {
            assert!(future.await.is_err());
        } else {
            drop(future);
        }
        assert!(pool.is_closed());
    }
}

// Test disposal while the pool is still open: closing a dedicated pool at the
// end alone would let the broken session contaminate acknowledgment draining.
#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn stopped_backend_discards_cancelled_begin_before_pool_closure(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use std::future::poll_fn;
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let admin = PgPool::connect(&db.admin_url).await.unwrap();
    let options: sqlx::postgres::PgConnectOptions = db.tasks_url.parse().unwrap();
    let mut barrier =
        super::begin_barrier::BeginBarrier::start(options.get_host(), options.get_port()).await;
    let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
        "return-during-drain",
        sqlx::postgres::PgPoolOptions::new().max_connections(1),
        options
            .host("127.0.0.1")
            .port(barrier.port())
            .ssl_mode(sqlx::postgres::PgSslMode::Disable),
    );
    let pool = lifecycle.pool().clone();
    let mut context = lifecycle.context().clone();
    context.start().unwrap();
    let queue =
        Queue::<u32>::new(pool.clone(), "return-during-drain", WorkerConfig::default()).unwrap();
    let mut storage = queue.storage();
    poll_fn(|cx| storage.poll_ready(cx, &context))
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    barrier.arm();
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            biased;
            () = barrier.reached() => {},
            result = poll_fn(|cx| storage.poll_next(cx, &context)) => panic!("fetch passed BEGIN: {result:?}"),
        }
    }).await.unwrap();
    context.stop().unwrap();
    poll_fn(|cx| storage.poll_close(cx, &context))
        .await
        .unwrap();
    barrier.release();
    assert!(
        !pool.is_closed(),
        "the pool must stay available during drain"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let alive: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT FROM pg_stat_activity WHERE pid=$1)")
                    .bind(pid)
                    .fetch_one(&admin)
                    .await
                    .unwrap();
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("shutdown returned the cancelled BEGIN session for reuse");
    drop(lifecycle);
    pool.close().await;
    tokio::time::timeout(Duration::from_secs(5), barrier.finish())
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
async fn overflowing_shutdown_deadline_returns_config_error_and_retires_pool() {
    for signal_ready in [false, true] {
        let lifecycle = hypgmx_tasks::worker::Lifecycle::new(
            "deadline-overflow",
            sqlx::postgres::PgPoolOptions::new(),
            "postgres://unused:unused@localhost/unused".parse().unwrap(),
        );
        let mut context = lifecycle.context().clone();
        context.start().unwrap();
        let pool = lifecycle.pool().clone();
        let worker = async {
            if signal_ready {
                std::future::pending::<()>().await;
            }
            Ok(())
        };
        let signal = async {
            if !signal_ready {
                std::future::pending::<()>().await;
            }
        };
        let result = lifecycle.run_until(worker, signal, Duration::MAX).await;
        assert!(matches!(result, Err(hypgm_errors::TasksError::Config(_))));
        assert!(context.is_shutting_down());
        assert!(pool.is_closed());
    }
}
