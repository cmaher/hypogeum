use exemplum_dmn_auth::repo::{AuthRepo, PgAuthRepo};
use exemplum_dmn_users::repo::{PgUserRepo, UserRepo};
use hypgm_test_support::TestDatabase;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use uuid::Uuid;

const TEST_JWT_SECRET: &str = "runtime-test-secret-that-is-over-32-bytes";

struct RunningApp {
    process: Child,
    endpoint: String,
}

#[test]
fn integration_suite() {
    hypgm_test_support::run_suite();
}

fn app_command(url: &str, bind: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_app"));
    command
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env_clear()
        .env("EXEMPLUM_ENV_FILE", "")
        .env("EXEMPLUM_APP_BIND", bind)
        .env("EXEMPLUM_APP_DATABASE_URL", url)
        .env("EXEMPLUM_APP_JWT_SECRET", TEST_JWT_SECRET)
        .env("EXEMPLUM_LOG", "off")
        .env("EXEMPLUM_APP_SESSION_CLEANUP_INTERVAL_SECONDS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn start_app(url: &str) -> RunningApp {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut process = app_command(url, &address.to_string()).spawn().unwrap();
    let endpoint = format!("http://{address}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(
                process.try_wait().unwrap().is_none(),
                "app exited during startup"
            );
            if let Ok(response) = client.get(format!("{endpoint}/healthz")).send().await
                && response.status().is_success()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("app did not become ready");
    RunningApp { process, endpoint }
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn executable_uses_persisted_credentials_sessions_and_configured_signing_key(
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
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    PgUserRepo::new(pool.clone())
        .create(user_id, "Runtime User")
        .await
        .unwrap();
    PgAuthRepo::new(pool.clone())
        .create(
            auth_id,
            user_id,
            "password",
            "runtime-user",
            &hypgm_authn::password::hash_password("password").unwrap(),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let RunningApp {
        mut process,
        endpoint,
    } = start_app(&db.users_url).await;
    sqlx::query("INSERT INTO exemplum.sessions (id, data, expiry_date) VALUES ('expired', $1, now() - interval '1 hour'), ('unexpired', $1, now() + interval '1 hour')")
        .bind(vec![0u8]).execute(&pool).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT FROM exemplum.sessions WHERE id='expired')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if !exists {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("executable did not delete expired sessions");
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT FROM exemplum.sessions WHERE id='unexpired')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(exists, "cleanup removed an active session");
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{endpoint}/api/auth/login"))
        .header("content-type", "application/json")
        .body(r#"{"username":"runtime-user","password":"password"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let response = client
        .get(format!("{endpoint}/api/hello/me"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["message"], "Hello, Runtime User!");
    let claims = hypgm_authn::Claims::new(
        auth_id,
        user_id,
        chrono::Utc::now().timestamp() as usize + 3600,
    );
    for (secret, expected) in [
        (
            b"exemplum-default-secret-key-32bytes!".as_slice(),
            reqwest::StatusCode::UNAUTHORIZED,
        ),
        (TEST_JWT_SECRET.as_bytes(), reqwest::StatusCode::OK),
    ] {
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret),
        )
        .unwrap();
        assert_eq!(
            client
                .get(format!("{endpoint}/api/hello/me"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    process.kill().await.unwrap();
    process.wait().await.unwrap();
    let RunningApp {
        mut process,
        endpoint,
    } = start_app(&db.users_url).await;
    assert_eq!(
        client
            .get(format!("{endpoint}/api/hello/me"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    process.kill().await.unwrap();
    process.wait().await.unwrap();
    // An old/incomplete schema must fail startup rather than serving with broken sessions.
    let admin = sqlx::PgPool::connect(&db.admin_url).await.unwrap();
    sqlx::query("DROP TABLE exemplum.sessions")
        .execute(&admin)
        .await
        .unwrap();
    let mut missing_sessions = app_command(&db.users_url, "127.0.0.1:0").spawn().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), missing_sessions.wait())
        .await
        .expect("app kept serving with a missing session table")
        .unwrap();
    assert!(!status.success());
    admin.close().await;
    pool.close().await;
}
