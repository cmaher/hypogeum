//! Runs the app executable against an isolated database and a stub of the
//! Kratos public API (readiness and whoami only).
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use hypgm_test_support::TestDatabase;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use uuid::Uuid;

const SESSION: &str = "ory_kratos_session=runtime-session";

#[test]
fn integration_suite() {
    hypgm_test_support::run_suite();
}

/// Serves `/health/ready` with `ready` and a whoami that accepts only
/// [`SESSION`] for `identity`.
async fn stub_kratos(ready: StatusCode, identity: Uuid) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/health/ready", get(move || async move { ready }))
        .route(
            "/sessions/whoami",
            get(move |headers: HeaderMap| async move {
                if headers.get("cookie").is_none_or(|cookie| cookie != SESSION) {
                    return (StatusCode::UNAUTHORIZED, "{}".to_owned());
                }
                let session = serde_json::json!({
                    "id": Uuid::new_v4(),
                    "active": true,
                    "identity": {
                        "id": identity,
                        "traits": {"username": "runtime-user", "display_name": "Runtime User"}
                    }
                });
                (StatusCode::OK, session.to_string())
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await });
    address
}

fn app_command(url: &str, kratos: SocketAddr, bind: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_app"));
    command
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env_clear()
        .env("EXEMPLUM_ENV_FILE", "")
        .env("EXEMPLUM_APP_BIND", bind)
        .env("EXEMPLUM_APP_DATABASE_URL", url)
        .env("EXEMPLUM_APP_KRATOS_PUBLIC_URL", format!("http://{kratos}"))
        .env("EXEMPLUM_LOG", "off")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn start_app(url: &str, kratos: SocketAddr) -> (Child, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut process = app_command(url, kratos, &address.to_string())
        .spawn()
        .unwrap();
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
    (process, endpoint)
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn executable_provisions_pending_accounts_and_serves_approved_sessions(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use exemplum_dmn_users::{
        accounts::{AccountRepo, ApprovalStatus},
        repo::PgAccountRepo,
    };
    let db = TestDatabase::provision(options).await;
    let config =
        hypgmx_rdbms::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
            .with_app_schema("exemplum")
            .with_domain_migrator(<dyn hypgmx_rdbms::DomainMigrator>::from_sea_orm::<
                exemplum_dmn_users::pg::migrations::Migrator,
            >());
    db.migrate_with(&config).await;
    let identity = Uuid::new_v4();

    // Startup fails closed while Kratos is not ready.
    let unready = stub_kratos(StatusCode::SERVICE_UNAVAILABLE, identity).await;
    let mut process = app_command(&db.users_url, unready, "127.0.0.1:0")
        .spawn()
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), process.wait())
        .await
        .expect("app started without a ready Kratos")
        .unwrap();
    assert!(!status.success());

    let kratos = stub_kratos(StatusCode::OK, identity).await;
    let (mut process, endpoint) = start_app(&db.users_url, kratos).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let me = |cookie: Option<&str>| {
        let mut request = client.get(format!("{endpoint}/api/hello/me"));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.send()
    };
    assert_eq!(me(None).await.unwrap().status(), 401);
    assert_eq!(
        me(Some("ory_kratos_session=unknown"))
            .await
            .unwrap()
            .status(),
        401
    );
    // The first signed-in request provisions a pending account.
    assert_eq!(me(Some(SESSION)).await.unwrap().status(), 401);
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = PgAccountRepo::new(pool.clone());
    let account = repo.get(identity).await.unwrap();
    assert_eq!(account.display_name, "Runtime User");
    assert_eq!(account.approval_status, ApprovalStatus::Pending);
    let page = client
        .get(format!("{endpoint}/"))
        .header("cookie", SESSION)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 303);
    assert_eq!(page.headers()["location"], "/registration-pending");

    repo.approve(identity).await.unwrap();
    let response = me(Some(SESSION)).await.unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["message"], "Hello, Runtime User!");
    process.kill().await.unwrap();
    process.wait().await.unwrap();
    pool.close().await;
}
