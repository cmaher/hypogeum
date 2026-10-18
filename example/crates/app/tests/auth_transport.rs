use exemplum_dmn_auth::repo::{AuthRepo, InMemoryAuthRepo};
use exemplum_dmn_auth::{LoginRequest, LoginResponse, TokenResponse};
use exemplum_dmn_hello_world::service::DefaultHelloWorldService;
use exemplum_dmn_users::models::{PageRequest, User, UserPage};
use exemplum_dmn_users::service::{UserError, UserService};
use hypgm_authn::api_key::{GeneratedApiKey, generate_api_key};
use hypgm_authn::password::hash_password;
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;

struct InMemoryUserService {
    users: RwLock<HashMap<Uuid, User>>,
}

impl InMemoryUserService {
    fn new() -> Self {
        Self {
            users: RwLock::new(HashMap::new()),
        }
    }

    fn add_user(&self, id: Uuid, display_name: &str) {
        let user = User {
            id,
            display_name: display_name.to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        self.users.write().unwrap().insert(id, user);
    }
}

#[async_trait::async_trait]
impl UserService for InMemoryUserService {
    async fn create(&self, name: &str) -> Result<User, UserError> {
        let id = Uuid::new_v4();
        self.add_user(id, name);
        self.get(id).await
    }

    async fn get(&self, id: Uuid) -> Result<User, UserError> {
        self.users
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(UserError::NotFound)
    }

    async fn list(&self, _page: PageRequest) -> Result<UserPage, UserError> {
        unimplemented!()
    }

    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, UserError> {
        self.add_user(id, name);
        self.get(id).await
    }

    async fn delete(&self, id: Uuid) -> Result<(), UserError> {
        self.users.write().unwrap().remove(&id);
        Ok(())
    }
}

struct TestHarness {
    endpoint: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<std::io::Result<()>>,
    pub auth_repo: Arc<InMemoryAuthRepo>,
    pub user_service: Arc<InMemoryUserService>,
}

impl TestHarness {
    async fn start_with_policy(policies: &str) -> Self {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let schema_path = manifest_dir.join("../../config/schema.cedarschema");
        let schema_content = std::fs::read_to_string(&schema_path)
            .unwrap_or_else(|_| panic!("Failed to read {}", schema_path.display()));

        let (schema, policies) = hypgm_authz::load_schema_and_policies(&schema_content, policies)
            .expect("Failed to load Cedar schema and policies");
        let authorizer = Arc::new(
            hypgm_authz::Authorizer::new(schema, policies).expect("valid authorization entities"),
        );

        let user_service = Arc::new(InMemoryUserService::new());
        let auth_repo = Arc::new(InMemoryAuthRepo::new());
        let token_config =
            exemplum_app::TokenConfig::new(b"test-jwt-secret-key-32-bytes-long!".to_vec());
        let session_store = tower_sessions::MemoryStore::default();

        let components = exemplum_app::AppComponents::new(
            Arc::new(DefaultHelloWorldService),
            user_service.clone(),
            auth_repo.clone(),
            authorizer,
            session_store,
            token_config,
            Default::default(),
        );

        let router = exemplum_app::router(components);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let serving = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });

        Self {
            endpoint,
            stop,
            serving,
            auth_repo,
            user_service,
        }
    }

    async fn start() -> Self {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let policies_path = manifest_dir.join("../../config/policies.cedar");
        let policy_content = std::fs::read_to_string(&policies_path)
            .unwrap_or_else(|_| panic!("Failed to read {}", policies_path.display()));
        Self::start_with_policy(&policy_content).await
    }

    async fn shutdown(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(5), self.serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn test_anonymous_hello_me_returns_401() {
    let harness = TestHarness::start().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    harness.shutdown().await;
}

#[tokio::test]
async fn test_login_session_cookie_and_hello_me() {
    let harness = TestHarness::start().await;
    let client = reqwest::Client::new();

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let display_name = "Alice Liddell";
    harness.user_service.add_user(user_id, display_name);

    let password = "SecretPassword123!";
    let password_hash = hash_password(password).unwrap();
    harness
        .auth_repo
        .create(
            auth_id,
            user_id,
            "password",
            "alice",
            &password_hash,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // Invalid login returns 401
    let bad_login = LoginRequest {
        username: "alice".to_string(),
        password: "WrongPassword".to_string(),
    };
    let resp = client
        .post(format!("{}/api/auth/login", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(&bad_login).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Valid login sets session cookie
    let good_login = LoginRequest {
        username: "alice".to_string(),
        password: password.to_string(),
    };
    let resp = client
        .post(format!("{}/api/auth/login", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(&good_login).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let cookie_header = resp
        .headers()
        .get(SET_COOKIE)
        .expect("Set-Cookie header must be present")
        .to_str()
        .unwrap()
        .to_string();

    let bytes = resp.bytes().await.unwrap();
    let login_res: LoginResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(login_res.auth_id, auth_id);
    assert_eq!(login_res.user_id, user_id);

    // Authenticated request with session cookie
    let resp = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(COOKIE, &cookie_header)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.bytes().await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["message"], format!("Hello, {display_name}!"));

    harness.shutdown().await;
}

#[tokio::test]
async fn test_two_distinct_users_receive_respective_display_names() {
    let harness = TestHarness::start().await;
    let client = reqwest::Client::new();

    // User 1
    let user1_id = Uuid::new_v4();
    let auth1_id = Uuid::new_v4();
    harness.user_service.add_user(user1_id, "User One");
    harness
        .auth_repo
        .create(
            auth1_id,
            user1_id,
            "password",
            "user1",
            &hash_password("Pass123!").unwrap(),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // User 2
    let user2_id = Uuid::new_v4();
    let auth2_id = Uuid::new_v4();
    harness.user_service.add_user(user2_id, "User Two");
    harness
        .auth_repo
        .create(
            auth2_id,
            user2_id,
            "password",
            "user2",
            &hash_password("Pass456!").unwrap(),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // Login User 1
    let resp1 = client
        .post(format!("{}/api/auth/login", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(r#"{"username":"user1","password":"Pass123!"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let cookie1 = resp1
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Login User 2
    let resp2 = client
        .post(format!("{}/api/auth/login", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(r#"{"username":"user2","password":"Pass456!"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let cookie2 = resp2
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // GET /api/hello/me for User 1
    let resp_me1 = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(COOKIE, &cookie1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_me1.status(), StatusCode::OK);
    let bytes1 = resp_me1.bytes().await.unwrap();
    let json1: serde_json::Value = serde_json::from_slice(&bytes1).unwrap();
    assert_eq!(json1["message"], "Hello, User One!");

    // GET /api/hello/me for User 2
    let resp_me2 = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(COOKIE, &cookie2)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_me2.status(), StatusCode::OK);
    let bytes2 = resp_me2.bytes().await.unwrap();
    let json2: serde_json::Value = serde_json::from_slice(&bytes2).unwrap();
    assert_eq!(json2["message"], "Hello, User Two!");

    harness.shutdown().await;
}

#[tokio::test]
async fn test_token_exchange_and_jwt_bearer_auth() {
    let harness = TestHarness::start().await;
    let client = reqwest::Client::new();

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    harness.user_service.add_user(user_id, "API Developer");

    let GeneratedApiKey {
        raw_key,
        secret_hash: key_hash,
    } = generate_api_key("exmpl_key");
    harness
        .auth_repo
        .create(
            auth_id,
            user_id,
            "api_key",
            &key_hash,
            &key_hash,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // Exchange invalid key returns 401
    let bad_token_resp = client
        .post(format!("{}/api/auth/token", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(r#"{"api_key":"exmpl_key_invalid"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_token_resp.status(), StatusCode::UNAUTHORIZED);

    // Exchange valid key returns token
    let good_token_resp = client
        .post(format!("{}/api/auth/token", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(format!(r#"{{"api_key":"{raw_key}"}}"#))
        .send()
        .await
        .unwrap();
    assert_eq!(good_token_resp.status(), StatusCode::OK);

    let bytes = good_token_resp.bytes().await.unwrap();
    let token_res: TokenResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(token_res.token_type, "Bearer");
    assert!(!token_res.access_token.is_empty());

    // GET /api/hello/me with Bearer JWT
    let resp = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(AUTHORIZATION, format!("Bearer {}", token_res.access_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.bytes().await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["message"], "Hello, API Developer!");

    harness.shutdown().await;
}

#[tokio::test]
async fn test_overriding_policy_with_forbid_returns_403() {
    let forbid_policy = r#"
permit(principal, action, resource);
forbid(principal, action == Action::"greet", resource);
"#;
    let harness = TestHarness::start_with_policy(forbid_policy).await;
    let client = reqwest::Client::new();

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    harness.user_service.add_user(user_id, "Forbidden User");
    let GeneratedApiKey {
        raw_key,
        secret_hash: key_hash,
    } = generate_api_key("exmpl_key");
    harness
        .auth_repo
        .create(
            auth_id,
            user_id,
            "api_key",
            &key_hash,
            &key_hash,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    let token_resp = client
        .post(format!("{}/api/auth/token", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(format!(r#"{{"api_key":"{raw_key}"}}"#))
        .send()
        .await
        .unwrap();
    assert_eq!(token_resp.status(), StatusCode::OK);
    let bytes = token_resp.bytes().await.unwrap();
    let token_res: TokenResponse = serde_json::from_slice(&bytes).unwrap();

    let resp = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(AUTHORIZATION, format!("Bearer {}", token_res.access_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    harness.shutdown().await;
}

#[tokio::test]
async fn test_logout_invalidates_session_and_clears_cookie() {
    let harness = TestHarness::start().await;
    let client = reqwest::Client::new();

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    harness.user_service.add_user(user_id, "Logout User");
    harness
        .auth_repo
        .create(
            auth_id,
            user_id,
            "password",
            "logout_user",
            &hash_password("Pass123!").unwrap(),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // Login
    let resp = client
        .post(format!("{}/api/auth/login", harness.endpoint))
        .header(CONTENT_TYPE, "application/json")
        .body(r#"{"username":"logout_user","password":"Pass123!"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Access /me succeeds
    let resp = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Logout
    let resp = client
        .post(format!("{}/api/auth/logout", harness.endpoint))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Access after logout returns 401
    let resp = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    harness.shutdown().await;
}

fn postgres_exemplum_config() -> hypgmx_rdbms::DatabaseConfig {
    hypgmx_rdbms::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
        .with_app_schema("exemplum")
        .with_domain_migrator(<dyn hypgmx_rdbms::DomainMigrator>::from_sea_orm::<
            exemplum_dmn_users::pg::migrations::Migrator,
        >())
}

#[test]
fn integration_suite() {
    hypgm_test_support::run_suite();
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn postgres_e2e_auth_transport(
    _pool: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    use exemplum_dmn_auth::repo::PgAuthRepo;
    use exemplum_dmn_users::repo::{PgUserRepo, UserRepo};
    use hypgm_test_support::TestDatabase;

    let db = TestDatabase::provision(options).await;
    let config = postgres_exemplum_config();
    db.migrate_with(&config).await;

    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let schema_path = manifest_dir.join("../../config/schema.cedarschema");
    let schema_content = std::fs::read_to_string(&schema_path)
        .unwrap_or_else(|_| panic!("Failed to read {}", schema_path.display()));
    let policies_path = manifest_dir.join("../../config/policies.cedar");
    let policy_content = std::fs::read_to_string(&policies_path)
        .unwrap_or_else(|_| panic!("Failed to read {}", policies_path.display()));

    let (schema, policies) =
        hypgm_authz::load_schema_and_policies(&schema_content, &policy_content)
            .expect("Failed to load Cedar schema and policies");
    let authorizer = Arc::new(
        hypgm_authz::Authorizer::new(schema, policies).expect("valid authorization entities"),
    );
    let token_config =
        exemplum_app::TokenConfig::new(b"test-jwt-secret-key-32-bytes-long!".to_vec());

    let components = exemplum_app::AppComponentsFactory::from_pool(
        pool.clone(),
        authorizer,
        token_config,
        Default::default(),
    );
    let router = exemplum_app::router(components);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });

    // 1. Create user in PostgreSQL database
    let user_repo = PgUserRepo::new(pool.clone());
    let user_id = Uuid::new_v4();
    let user = user_repo.create(user_id, "Postgres User").await.unwrap();

    // 2. Create password credential identity
    let auth_repo = PgAuthRepo::new(pool.clone());
    let auth_id = Uuid::new_v4();
    let password_hash = hash_password("secret-password").unwrap();
    auth_repo
        .create(
            auth_id,
            user.id,
            "password",
            "pguser",
            &password_hash,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // 3. Login with password and get session cookie
    let client = reqwest::Client::new();
    let login_req = LoginRequest {
        username: "pguser".to_string(),
        password: "secret-password".to_string(),
    };
    let login_resp = client
        .post(format!("{endpoint}/api/auth/login"))
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(&login_req).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::OK);

    let cookie_header = login_resp
        .headers()
        .get(SET_COOKIE)
        .expect("Set-Cookie header must be present")
        .to_str()
        .unwrap()
        .to_string();

    // 4. Verify session is persisted in exemplum.sessions table in PostgreSQL
    let session_count: i64 = sqlx::query_scalar("SELECT count(*) FROM exemplum.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        session_count >= 1,
        "Session must be persisted in PostgreSQL sessions table"
    );

    // 5. Access /api/hello/me with session cookie
    let me_resp = client
        .get(format!("{endpoint}/api/hello/me"))
        .header(COOKIE, &cookie_header)
        .send()
        .await
        .unwrap();
    assert_eq!(me_resp.status(), StatusCode::OK);
    let me_bytes = me_resp.bytes().await.unwrap();
    let me_body: serde_json::Value = serde_json::from_slice(&me_bytes).unwrap();
    assert_eq!(me_body["message"], "Hello, Postgres User!");

    // 6. Create API key in PostgreSQL database
    let api_key_auth_id = Uuid::new_v4();
    let GeneratedApiKey {
        raw_key,
        secret_hash: key_hash,
    } = generate_api_key("exmpl_key");
    auth_repo
        .create(
            api_key_auth_id,
            user.id,
            "api_key",
            &key_hash,
            &key_hash,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // 7. Exchange API key for JWT token via POST /api/auth/token
    let token_req = serde_json::json!({
        "grant_type": "api_key",
        "api_key": raw_key,
    });
    let token_resp = client
        .post(format!("{endpoint}/api/auth/token"))
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(&token_req).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(token_resp.status(), StatusCode::OK);
    let token_bytes = token_resp.bytes().await.unwrap();
    let token_body: TokenResponse = serde_json::from_slice(&token_bytes).unwrap();

    // 8. Access /api/hello/me with Bearer token
    let bearer_resp = client
        .get(format!("{endpoint}/api/hello/me"))
        .header(AUTHORIZATION, format!("Bearer {}", token_body.access_token))
        .send()
        .await
        .unwrap();
    assert_eq!(bearer_resp.status(), StatusCode::OK);
    let bearer_bytes = bearer_resp.bytes().await.unwrap();
    let bearer_body: serde_json::Value = serde_json::from_slice(&bearer_bytes).unwrap();
    assert_eq!(bearer_body["message"], "Hello, Postgres User!");

    // 9. Logout session
    let logout_resp = client
        .post(format!("{endpoint}/api/auth/logout"))
        .header(COOKIE, &cookie_header)
        .send()
        .await
        .unwrap();
    assert_eq!(logout_resp.status(), StatusCode::OK);

    // Verify session removed from PostgreSQL sessions table
    let session_count_after: i64 = sqlx::query_scalar("SELECT count(*) FROM exemplum.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(session_count_after, 0);

    let _ = stop.send(());
    let _ = serving.await;
    pool.close().await;
}
