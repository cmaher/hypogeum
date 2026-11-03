//! Runs the app executable against an isolated database and either a stub of
//! the Kratos public API (readiness and whoami only) or a real Kratos container
//! configured from `infra/kratos/`.
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

/// Provisions the test database and applies the Exemplum migrations.
async fn migrated_database(options: sqlx::postgres::PgConnectOptions) -> TestDatabase {
    let db = TestDatabase::provision(options).await;
    let config =
        hypgmx_rdbms::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
            .with_app_schema("exemplum")
            .with_domain_migrator(<dyn hypgmx_rdbms::DomainMigrator>::from_sea_orm::<
                exemplum_dmn_users::pg::migrations::Migrator,
            >());
    db.migrate_with(&config).await;
    db
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
    let db = migrated_database(options).await;
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

/// Browser origin baked into `infra/kratos/kratos.yml`; Kratos builds flow
/// actions and logout URLs from it. The test calls Kratos directly instead.
const KRATOS_BROWSER_URL: &str = "http://localhost:8080/.ory/kratos";

/// Starts Kratos v1.3.1 with the project configuration against a fresh
/// database in the suite's PostgreSQL container, migrating before serving.
/// Returns the container (removed on drop) and its public API endpoint.
async fn start_kratos(
    options: &sqlx::postgres::PgConnectOptions,
) -> (
    testcontainers_modules::testcontainers::ContainerAsync<
        testcontainers_modules::testcontainers::GenericImage,
    >,
    String,
) {
    use testcontainers_modules::testcontainers::{
        GenericImage, ImageExt,
        core::{Host, IntoContainerPort},
        runners::AsyncRunner,
    };
    let admin = sqlx::PgPool::connect_with(options.clone()).await.unwrap();
    // Mirrors infra/postgres/kratos.sql: a login role owning only its database.
    // Separate statements: CREATE DATABASE cannot run in a multi-statement block.
    for statement in [
        "CREATE ROLE kratos_e2e LOGIN PASSWORD 'test-kratos' \
         NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT",
        "CREATE DATABASE kratos_e2e OWNER kratos_e2e",
    ] {
        sqlx::raw_sql(statement).execute(&admin).await.unwrap();
    }
    admin.close().await;
    // Kratos reaches the suite's PostgreSQL through its published host port.
    let dsn = format!(
        "postgres://kratos_e2e:test-kratos@host.docker.internal:{}/kratos_e2e?sslmode=disable",
        options.get_port()
    );
    let config = |file| {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../infra/kratos")
                .join(file),
        )
        .unwrap()
    };
    let container = GenericImage::new("oryd/kratos", "v1.3.1")
        .with_exposed_port(4433.tcp())
        .with_entrypoint("/bin/sh")
        .with_cmd([
            "-c",
            "kratos migrate sql --read-from-env --yes && \
             exec kratos serve --config /etc/config/kratos/kratos.yml",
        ])
        .with_host("host.docker.internal", Host::HostGateway)
        .with_env_var("DSN", dsn)
        .with_env_var("SECRETS_COOKIE", "e2e-cookie-secret-0123456789")
        .with_env_var("SECRETS_CIPHER", "e2e-cipher-secret-0123456789abcd")
        .with_copy_to("/etc/config/kratos/kratos.yml", config("kratos.yml"))
        .with_copy_to(
            "/etc/config/kratos/identity.schema.json",
            config("identity.schema.json"),
        )
        .start()
        .await
        .expect("Start Kratos testcontainer");
    let port = container.get_host_port_ipv4(4433).await.unwrap();
    let endpoint = format!("http://127.0.0.1:{port}");
    let client = hypgmx_kratos::KratosClient::new(hypgmx_kratos::KratosConfig {
        public_url: endpoint.parse().unwrap(),
        request_timeout: hypgmx_kratos::DEFAULT_REQUEST_TIMEOUT,
    })
    .unwrap();
    tokio::time::timeout(Duration::from_secs(90), async {
        while client.ready().await.is_err() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("Kratos did not become ready");
    (container, endpoint)
}

/// `name=value` pairs from `Set-Cookie` headers, ignoring attributes. Kratos
/// marks its cookies `Secure`, which a cookie jar would withhold over http.
fn set_cookies(response: &reqwest::Response, jar: &mut Vec<(String, String)>) {
    for value in response.headers().get_all("set-cookie") {
        let pair = value.to_str().unwrap().split(';').next().unwrap();
        let (name, value) = pair.split_once('=').unwrap();
        jar.retain(|(existing, _)| existing != name);
        jar.push((name.to_owned(), value.to_owned()));
    }
}

fn cookie_header(jar: &[(String, String)]) -> String {
    jar.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn json(response: reqwest::Response) -> serde_json::Value {
    let status = response.status();
    let body = response.bytes().await.unwrap();
    serde_json::from_slice(&body)
        .unwrap_or_else(|_| panic!("{status}: {}", String::from_utf8_lossy(&body)))
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn kratos_registration_reaches_the_app_after_approval_until_logout(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use exemplum_dmn_users::{
        accounts::{AccountRepo, ApprovalStatus},
        repo::PgAccountRepo,
    };
    let db = migrated_database(options.clone()).await;
    let (_kratos_container, kratos) = start_kratos(&options).await;
    let kratos_address: SocketAddr = kratos.trim_start_matches("http://").parse().unwrap();
    let (mut process, endpoint) = start_app(&db.users_url, kratos_address).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Register through the Kratos browser flow (JSON variant) so the session
    // arrives as the `ory_kratos_session` cookie the app reads.
    let mut jar = Vec::new();
    let response = client
        .get(format!("{kratos}/self-service/registration/browser"))
        .header("accept", "application/json")
        .send()
        .await
        .unwrap();
    set_cookies(&response, &mut jar);
    let flow = json(response).await;
    let csrf = flow["ui"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["attributes"]["name"] == "csrf_token")
        .map(|node| node["attributes"]["value"].as_str().unwrap().to_owned())
        .unwrap();
    let username = format!("e2e-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let display_name = "End To End";
    let body = serde_json::json!({
        "method": "password",
        "csrf_token": csrf,
        // Random, so the breached-password check passes; bcrypt caps at 72 bytes.
        "password": format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        "traits": {"username": username, "display_name": display_name},
    });
    let response = client
        .post(format!(
            "{kratos}/self-service/registration?flow={}",
            flow["id"].as_str().unwrap()
        ))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("cookie", cookie_header(&jar))
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status();
    set_cookies(&response, &mut jar);
    let registered = json(response).await;
    assert_eq!(status, 200, "registration failed: {registered}");
    let identity: Uuid = registered["identity"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let session = jar
        .iter()
        .find(|(name, _)| name == "ory_kratos_session")
        .map(|(name, value)| format!("{name}={value}"))
        .expect("registration sets the session cookie");

    let me = || {
        client
            .get(format!("{endpoint}/api/hello/me"))
            .header("cookie", &session)
            .send()
    };
    // A new registration is pending: the API treats it as unauthenticated.
    assert_eq!(me().await.unwrap().status(), 401);
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = PgAccountRepo::new(pool.clone());
    let account = repo.get(identity).await.unwrap();
    assert_eq!(account.display_name, display_name);
    assert_eq!(account.approval_status, ApprovalStatus::Pending);

    // `exemplum-admin promote` path: approve by the registered username.
    let promoted = repo.promote_admin(&username).await.unwrap();
    assert_eq!(promoted.id, identity);
    let response = me().await.unwrap();
    assert_eq!(response.status(), 200);
    // The handler loads the account by `Principal::user_id`, the identity id.
    assert_eq!(
        json(response).await["message"],
        format!("Hello, {display_name}!")
    );

    // POST /logout redirects to the Kratos logout URL; following it ends the session.
    let response = client
        .post(format!("{endpoint}/logout"))
        .header("cookie", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let logout_path = location
        .strip_prefix(KRATOS_BROWSER_URL)
        .expect("logout URL is under the browser URL");
    let response = client
        .get(format!("{kratos}{logout_path}"))
        .header("accept", "application/json")
        .header("cookie", cookie_header(&jar))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    assert_eq!(me().await.unwrap().status(), 401);

    process.kill().await.unwrap();
    process.wait().await.unwrap();
    pool.close().await;
}
