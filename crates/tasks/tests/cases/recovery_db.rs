//! Fixture-only effects and processes. No domain submission API or production handler.
use apalis::prelude::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use sqlx::{Connection, PgConnection, PgPool};
use std::{
    process::{Child, Command},
    time::Duration,
};
use tasks::{Queue, WorkerConfig, board::Board};
use tower::ServiceExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Effect {
    key: String,
}
fn config() -> WorkerConfig {
    WorkerConfig {
        heartbeat_interval: Duration::from_secs(1),
        missed_heartbeats: 2,
        ..Default::default()
    }
}
async fn pool(key: &str) -> PgPool {
    PgPool::connect(&std::env::var(key).unwrap()).await.unwrap()
}
async fn wait_status(queue: &Queue<Effect>, id: &TaskId, status: Status) {
    let mut storage = queue.storage();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if storage
                .fetch_by_id(id)
                .await
                .unwrap()
                .is_some_and(|task| task.status() == status)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn worker_process(hold: bool, url: &str) -> Process {
    Process(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "recovery::fixture_worker_process",
                "--nocapture",
            ])
            .env(
                "FOUNDATION_TASK_FIXTURE_CHILD",
                if hold { "hold" } else { "run" },
            )
            .env("TASKS_FIXTURE_DATABASE_URL", url)
            .spawn()
            .unwrap(),
    )
}

#[tokio::test]
#[ignore = "child fixture, launched by recovery_acceptance"]
async fn fixture_worker_process() {
    let Ok(mode) = std::env::var("FOUNDATION_TASK_FIXTURE_CHILD") else {
        return;
    };
    let pool = pool("TASKS_FIXTURE_DATABASE_URL").await;
    let queue = Queue::<Effect>::new(pool.clone(), "fixture-recovery", config()).unwrap();
    let lifecycle = tasks::worker::Lifecycle::new(
        &format!("fixture-process-{}", std::process::id()),
        sqlx::postgres::PgPoolOptions::new().max_connections(2),
        std::env::var("TASKS_FIXTURE_DATABASE_URL")
            .unwrap()
            .parse()
            .unwrap(),
    );
    let worker = queue
        .worker(&lifecycle)
        .await
        .unwrap()
        .build(move |effect: Effect| {
            let pool = pool.clone();
            let hold = mode == "hold";
            execute_recovery(effect, pool, hold)
        });
    lifecycle
        .run_until(
            worker.run(),
            std::future::pending(),
            config().shutdown_timeout,
        )
        .await
        .unwrap();
}

async fn atomicity(pool: &PgPool, admin: &PgPool, url: &str) {
    let queue = Queue::<Effect>::new(pool.clone(), "fixture-atomic", config()).unwrap();
    for outcome in ["commit", "rollback", "disconnect"] {
        let mut connection = PgConnection::connect(url).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        let mut tx = connection.begin().await.unwrap();
        sqlx::query("INSERT INTO task_fixture.submissions VALUES($1)")
            .bind(outcome)
            .execute(&mut *tx)
            .await
            .unwrap();
        let task = queue.task(Effect {
            key: outcome.into(),
        });
        let id = task.task_id().unwrap().clone();
        let compact = task.try_map_args(|args| serde_json::to_vec(&args)).unwrap();
        tasks::apalis_postgres::queries::push_tasks(&mut *tx, "fixture-atomic", vec![compact])
            .await
            .unwrap();
        match outcome {
            "commit" => tx.commit().await.unwrap(),
            "rollback" => tx.rollback().await.unwrap(),
            _ => {
                sqlx::query("SELECT pg_terminate_backend($1)")
                    .bind(pid)
                    .execute(admin)
                    .await
                    .unwrap();
                assert!(sqlx::query("SELECT 1").execute(&mut *tx).await.is_err());
                drop(tx);
            }
        }
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_fixture.submissions WHERE key=$1")
                .bind(outcome)
                .fetch_one(admin)
                .await
                .unwrap();
        assert_eq!(rows, i64::from(outcome == "commit"));
        assert_eq!(
            queue.storage().fetch_by_id(&id).await.unwrap().is_some(),
            outcome == "commit"
        );
    }
}

async fn board_inspection(queue: &Queue<Effect>, id: &TaskId) {
    let router = Board::new().register(queue).unwrap().router();
    let task = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/queues/fixture-recovery/tasks/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(task.status(), 200);
    let bytes = task.into_body().collect().await.unwrap().to_bytes();
    let task: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let text = task.to_string();
    assert!(text.contains(&id.to_string()));
    assert!(text.contains("Done"));
    assert!(task["ctx"].get("attempt").is_some());
    let workers = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/queues/fixture-recovery/workers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(workers.status(), 200);
    let bytes = workers.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("fixture-process-"));
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn recovery_acceptance(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let pool = PgPool::connect(&db.tasks_url).await.unwrap();
    let admin = PgPool::connect(&db.admin_url).await.unwrap();
    sqlx::raw_sql("CREATE SCHEMA task_fixture; CREATE TABLE task_fixture.submissions(key text PRIMARY KEY); CREATE TABLE task_fixture.recovery(key text PRIMARY KEY,executions integer NOT NULL); GRANT USAGE ON SCHEMA task_fixture TO foundation_tasks; GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA task_fixture TO foundation_tasks;").execute(&admin).await.unwrap();
    atomicity(&pool, &admin, &db.tasks_url).await;
    let queue = Queue::<Effect>::new(pool.clone(), "fixture-recovery", config()).unwrap();
    let task = queue.task(Effect {
        key: "recover".into(),
    });
    let id = task.task_id().unwrap().clone();
    queue.storage().push_task(task).await.unwrap();
    let first = worker_process(true, &db.tasks_url);
    tokio::time::timeout(Duration::from_secs(15), wait_started(&admin))
        .await
        .unwrap();
    drop(first); // An actual OS process dies while its task is in flight.
    let replacement = worker_process(false, &db.tasks_url);
    wait_status(&queue, &id, Status::Done).await;
    let effects: i32 =
        sqlx::query_scalar("SELECT executions FROM task_fixture.recovery WHERE key='recover'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(effects, 2);
    board_inspection(&queue, &id).await;
    drop(replacement);
    pool.close().await;
    admin.close().await;
}

async fn execute_recovery(effect: Effect, pool: PgPool, hold: bool) -> Result<(), BoxDynError> {
    sqlx::query("INSERT INTO task_fixture.recovery VALUES($1,1) ON CONFLICT(key) DO UPDATE SET executions=task_fixture.recovery.executions+1").bind(effect.key).execute(&pool).await?;
    if hold {
        std::future::pending::<()>().await;
    }
    Ok(())
}

async fn wait_started(admin: &PgPool) {
    loop {
        let started: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT FROM task_fixture.recovery WHERE key='recover')",
        )
        .fetch_one(admin)
        .await
        .unwrap();
        if started {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
