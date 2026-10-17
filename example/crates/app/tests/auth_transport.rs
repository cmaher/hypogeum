use exemplum_app::auth::password::{generate_api_key, hash_password};
use exemplum_app::auth::repo::{AuthRepo, InMemoryAuthRepo};
use exemplum_app::auth::{LoginRequest, LoginResponse, TokenResponse};
use exemplum_dmn_hello_world::service::DefaultHelloWorldService;
use exemplum_dmn_users::models::{PageRequest, User, UserPage};
use exemplum_dmn_users::service::{UserError, UserService};
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

        let authorizer = Arc::new(
            hypgm_authz::load_schema_and_policies(&schema_content, policies)
                .expect("Failed to load Cedar schema and policies"),
        );

        let user_service = Arc::new(InMemoryUserService::new());
        let auth_repo = Arc::new(InMemoryAuthRepo::new());
        let session_store = tower_sessions::MemoryStore::default();
        let jwt_secret = b"test-jwt-secret-key-32-bytes-long!".to_vec();

        let router = exemplum_app::router_with_auth(
            Arc::new(DefaultHelloWorldService),
            authorizer,
            user_service.clone(),
            auth_repo.clone(),
            session_store,
            jwt_secret,
        );

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

    let login_res: LoginResponse = resp.json().await.unwrap();
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

    let json: serde_json::Value = resp.json().await.unwrap();
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
    let json1: serde_json::Value = resp_me1.json().await.unwrap();
    assert_eq!(json1["message"], "Hello, User One!");

    // GET /api/hello/me for User 2
    let resp_me2 = client
        .get(format!("{}/api/hello/me", harness.endpoint))
        .header(COOKIE, &cookie2)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_me2.status(), StatusCode::OK);
    let json2: serde_json::Value = resp_me2.json().await.unwrap();
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

    let (raw_key, key_hash) = generate_api_key();
    harness
        .auth_repo
        .create(
            auth_id,
            user_id,
            "api_key",
            &raw_key,
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

    let token_res: TokenResponse = good_token_resp.json().await.unwrap();
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

    let json: serde_json::Value = resp.json().await.unwrap();
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
    let (raw_key, key_hash) = generate_api_key();
    harness
        .auth_repo
        .create(
            auth_id,
            user_id,
            "api_key",
            &raw_key,
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
    let token_res: TokenResponse = token_resp.json().await.unwrap();

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
