use exemplum_dmn_users::{
    accounts::AccountRepo,
    jobs::{WelcomeJob, welcome_queue},
    repo::PgAccountRepo,
};
use hypgm_test_support::TestDatabase;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::{Child, Command};
use uuid::Uuid;

#[test]
fn integration_suite() {
    // Build the sibling executable before the suite launches its database child.
    assert!(
        std::process::Command::new("cargo")
            .args([
                "build",
                "--locked",
                "-p",
                "exemplum-app-cli",
                "--bin",
                "app"
            ])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .unwrap()
            .success()
    );
    hypgm_test_support::run_suite();
}

fn command(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env_clear()
        .env("EXEMPLUM_ENV_FILE", "")
        .env("EXEMPLUM_LOG", "off")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    command
}
fn worker(db: &TestDatabase) -> Child {
    command(Path::new(env!("CARGO_BIN_EXE_exemplum-tasks")))
        .env("EXEMPLUM_TASKS_DATABASE_URL", &db.exemplum_tasks_url)
        .env("EXEMPLUM_TASKS_HEARTBEAT_SECONDS", "1")
        .env("EXEMPLUM_TASKS_MAX_ATTEMPTS", "7")
        .env("EXEMPLUM_TASKS_BACKOFF_INITIAL_SECONDS", "1")
        .env("EXEMPLUM_TASKS_BACKOFF_MAX_SECONDS", "1")
        .env("EXEMPLUM_TASKS_HANDLER_DATABASE_URL", &db.users_url)
        .spawn()
        .unwrap()
}
async fn stop(child: &mut Child) {
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
async fn completed(
    repo: &PgAccountRepo,
    id: Uuid,
    worker: &mut Child,
) -> exemplum_dmn_users::accounts::Account {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(worker.try_wait().unwrap().is_none(), "worker exited");
            let account = repo.get(id).await.unwrap();
            if account.welcome_processed_at.is_some() {
                return account;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("worker did not persist welcome timestamp")
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn registration_approval_and_durable_worker(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = TestDatabase::provision(options).await;
    let config =
        hypgmx_rdbms::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
            .with_app_schema("exemplum")
            .with_domain_migrator(<dyn hypgmx_rdbms::DomainMigrator>::from_sea_orm::<
                exemplum_dmn_users::pg::migrations::Migrator,
            >());
    db.migrate_with(&config).await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let admin = sqlx::PgPool::connect(&db.admin_url).await.unwrap();
    let repo = PgAccountRepo::new(pool.clone());
    repo.create_admin(
        "Administrator",
        "admin",
        &hypgm_authn::password::hash_password("admin-password-123").unwrap(),
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let app_binary = Path::new(env!("CARGO_BIN_EXE_exemplum-tasks")).with_file_name("app");
    let mut app = command(&app_binary)
        .env("EXEMPLUM_APP_BIND", address.to_string())
        .env("EXEMPLUM_APP_DATABASE_URL", &db.users_url)
        .env(
            "EXEMPLUM_APP_JWT_SECRET",
            "process-test-secret-more-than-32-characters",
        )
        .spawn()
        .unwrap();
    let endpoint = format!("http://{address}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(app.try_wait().unwrap().is_none(), "app exited");
            if let Ok(response) = client.get(format!("{endpoint}/healthz")).send().await
                && response.status().is_success()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let registered = client.post(format!("{endpoint}/api/auth/register")).json(&serde_json::json!({"display_name":"Process User", "username":"process-user", "password":"process-password-123"})).send().await.unwrap();
    assert_eq!(registered.status(), reqwest::StatusCode::CREATED);
    let id: Uuid = registered.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let login = client
        .post(format!("{endpoint}/api/auth/login"))
        .json(&serde_json::json!({"username":"admin","password":"admin-password-123"}))
        .send()
        .await
        .unwrap();
    assert!(login.status().is_success());
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let approval = format!("{endpoint}/api/admin/users/{id}/approve");
    // HTTP approval failure must roll back its account transition.
    sqlx::query("REVOKE INSERT ON apalis.jobs FROM exemplum_users")
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        client
            .post(&approval)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(repo.get(id).await.unwrap().approved_at.is_none());
    sqlx::query("GRANT INSERT ON apalis.jobs TO exemplum_users")
        .execute(&admin)
        .await
        .unwrap();
    let approve = || client.post(&approval).header("cookie", &cookie).send();
    let (a, b) = tokio::join!(approve(), approve());
    assert!(a.unwrap().status().is_success());
    assert!(b.unwrap().status().is_success());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM apalis.jobs WHERE job_type='exemplum.welcome.v1'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert!(repo.get(id).await.unwrap().welcome_processed_at.is_none());
    // Queue survives app shutdown and a stopped worker; no UI requests drive completion.
    stop(&mut app).await;
    // Sequence increments survive failed transactions: force five handler failures
    // and prove the worker's configured seventh attempt budget reaches success.
    sqlx::raw_sql(
        "CREATE SEQUENCE exemplum.welcome_test_attempts;
        GRANT USAGE ON SEQUENCE exemplum.welcome_test_attempts TO exemplum_users;
        CREATE FUNCTION exemplum.fail_first_five_welcomes() RETURNS trigger
        LANGUAGE plpgsql AS $$ BEGIN
            IF NEW.welcome_processed_at IS NOT NULL AND OLD.welcome_processed_at IS NULL
               AND nextval('exemplum.welcome_test_attempts') <= 5 THEN
                RAISE EXCEPTION 'simulated transient welcome failure';
            END IF;
            RETURN NEW;
        END $$;
        CREATE TRIGGER fail_first_five_welcomes BEFORE UPDATE ON exemplum.users
        FOR EACH ROW EXECUTE FUNCTION exemplum.fail_first_five_welcomes();",
    )
    .execute(&admin)
    .await
    .unwrap();
    let mut consumer = worker(&db);
    let first = completed(&repo, id, &mut consumer).await;
    let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM exemplum.welcome_test_attempts")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(attempts, 6);
    assert!(first.welcome_scheduled_at.is_some());
    sqlx::query("DROP TRIGGER fail_first_five_welcomes ON exemplum.users")
        .execute(&admin)
        .await
        .unwrap();
    // The handler persists before Apalis acknowledges; wait for acknowledgement
    // before stopping so the restart assertion is independent of that race.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let done: i64 = sqlx::query_scalar("SELECT count(*) FROM apalis.jobs WHERE job_type='exemplum.welcome.v1' AND status='Done'")
                .fetch_one(&admin).await.unwrap();
            if done == 1 { break; }
            assert!(consumer.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.expect("first delivery was not acknowledged");
    stop(&mut consumer).await;
    let queue = welcome_queue(
        pool.clone(),
        hypgmx_tasks::WorkerConfig::default().with_tasks_role("exemplum_tasks"),
    )
    .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL search_path = pg_catalog, apalis")
        .execute(&mut *transaction)
        .await
        .unwrap();
    let task = queue
        .task(WelcomeJob { user_id: id })
        .try_map_args(|args| serde_json::to_vec(&args))
        .unwrap();
    hypgmx_tasks::apalis_postgres::queries::push_tasks(
        &mut *transaction,
        exemplum_dmn_users::jobs::WELCOME_QUEUE,
        vec![task],
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let mut consumer = worker(&db);
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let done: i64 = sqlx::query_scalar("SELECT count(*) FROM apalis.jobs WHERE job_type='exemplum.welcome.v1' AND status='Done'").fetch_one(&admin).await.unwrap();
            if done == 2 { break; }
            assert!(consumer.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.expect("duplicate delivery was not completed after restart");
    assert_eq!(
        repo.get(id).await.unwrap().welcome_processed_at,
        first.welcome_processed_at
    );
    // Optional real-browser run uses this same isolated database and live worker.
    if std::env::var_os("EXEMPLUM_TEST_BROWSER").is_some() {
        let mut app = command(&app_binary)
            .env("EXEMPLUM_APP_BIND", address.to_string())
            .env("EXEMPLUM_APP_DATABASE_URL", &db.users_url)
            .env(
                "EXEMPLUM_APP_JWT_SECRET",
                "process-test-secret-more-than-32-characters",
            )
            .spawn()
            .unwrap();
        let status = Command::new("npx")
            .args(["playwright", "test", "--config", "registration.config.mjs"])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/browser"))
            .env("REGISTRATION_BASE_URL", &endpoint)
            .status()
            .await
            .unwrap();
        stop(&mut app).await;
        assert!(status.success(), "registration browser checks failed");
    }
    stop(&mut consumer).await;
    admin.close().await;
    pool.close().await;
}
