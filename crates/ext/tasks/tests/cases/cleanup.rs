use apalis::prelude::*;
use hypgmx_tasks::worker::Lifecycle;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use std::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn stalled_pool_cleanup_obeys_shutdown_deadline(
    _options: PgPoolOptions,
    options: PgConnectOptions,
) {
    let db = hypgm_test_support::TestDatabase::provision(options).await;
    for mode in ["signal", "deadline", "completed", "error", "cancel-cleanup"] {
        let options: PgConnectOptions = db.tasks_url.parse().unwrap();
        let mut barrier =
            super::begin_barrier::BeginBarrier::start_idle(options.get_host(), options.get_port())
                .await;
        let lifecycle = Lifecycle::new(
            "cleanup-deadline",
            PgPoolOptions::new().max_connections(1),
            options
                .host("127.0.0.1")
                .port(barrier.port())
                .ssl_mode(PgSslMode::Disable),
        );
        let mut context = lifecycle.context().clone();
        context.start().unwrap();
        let pool = lifecycle.pool().clone();
        // Returning a normal query's connection starts a background ping. Hold
        // its idle reply so it retains a pool permit throughout shutdown.
        sqlx::query("SELECT 1").execute(&pool).await.unwrap();
        barrier.arm();
        tokio::time::timeout(Duration::from_secs(5), barrier.reached())
            .await
            .unwrap();

        // Control only the shutdown clock, after all database setup completes.
        tokio::time::pause();
        let worker = async {
            match mode {
                "signal" => tokio::time::sleep(Duration::from_millis(60)).await,
                "deadline" => std::future::pending::<()>().await,
                "error" => return Err(WorkerError::BackendError("fixture failure".into())),
                _ => {}
            }
            Ok(())
        };
        let signal = async {
            if !matches!(mode, "signal" | "deadline") {
                std::future::pending::<()>().await;
            }
        };
        let mut future = Box::pin(lifecycle.run_until(worker, signal, Duration::from_millis(100)));
        poll_fn(|cx| {
            assert!(
                future.as_mut().poll(cx).is_pending(),
                "cleanup did not wait for the held ping"
            );
            Poll::Ready(())
        })
        .await;
        if mode == "cancel-cleanup" {
            assert!(pool.is_closed());
            drop(future);
        } else {
            if mode == "signal" {
                tokio::time::advance(Duration::from_millis(61)).await;
                poll_fn(|cx| {
                    assert!(future.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                assert!(
                    pool.is_closed(),
                    "worker must be dropped and pool closed after draining"
                );
                // Cleanup gets only the remaining 39ms, not a fresh 100ms.
                tokio::time::advance(Duration::from_millis(40)).await;
            } else {
                tokio::time::advance(Duration::from_millis(101)).await;
            }
            let result = poll_fn(|cx| match future.as_mut().poll(cx) {
                Poll::Ready(result) => Poll::Ready(result),
                Poll::Pending => panic!("pool cleanup exceeded shutdown deadline ({mode})"),
            })
            .await;
            assert!(matches!(
                result,
                Err(hypgm_errors::TasksError::DrainTimeout)
            ));
            assert!(pool.is_closed());
            assert!(
                pool.acquire().await.is_err(),
                "a retired pool must reject new acquisitions"
            );
            drop(future);
        }
        tokio::time::resume();
        barrier.release();
        // Cleanup must continue after timeout/cancellation without the caller
        // having to invoke close() again.
        tokio::time::timeout(Duration::from_secs(5), barrier.finish())
            .await
            .unwrap();
        assert_eq!(pool.size(), 0);
    }
}
