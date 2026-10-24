use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use exemplum_dmn_auth::{
    AuthComponentsFactory, AuthRepo, ExemplumAuthBackend, InMemoryAuthRepo, RegistrationRequest,
};
use exemplum_dmn_users::{accounts::*, models::PageRequest};
use hypgm_authn::{
    password::verify_password,
    session::{AuthBackend, PrincipalAuthBackend},
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

struct Accounts {
    account: Mutex<Option<Account>>,
    repo: Arc<InMemoryAuthRepo>,
}
#[async_trait]
impl AccountService for Accounts {
    async fn register(
        &self,
        name: &str,
        username: &str,
        hash: &str,
    ) -> Result<Account, AccountError> {
        assert!(verify_password("long password!", hash).unwrap());
        if self.account.lock().unwrap().is_some() {
            return Err(AccountError::DuplicateUsername);
        }
        let account = Account {
            id: uuid::Uuid::new_v4(),
            display_name: name.into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            approval_status: ApprovalStatus::Pending,
            is_admin: false,
            approved_at: None,
            welcome_scheduled_at: None,
            welcome_processed_at: None,
        };
        self.repo
            .create(
                uuid::Uuid::new_v4(),
                account.id,
                "password",
                username,
                hash,
                serde_json::json!({}),
            )
            .await
            .unwrap();
        *self.account.lock().unwrap() = Some(account.clone());
        Ok(account)
    }
    async fn approve(&self, _: uuid::Uuid) -> Result<Account, AccountError> {
        Err(AccountError::Validation)
    }
    async fn get(&self, _: uuid::Uuid) -> Result<Account, AccountError> {
        self.account
            .lock()
            .unwrap()
            .clone()
            .ok_or(AccountError::Validation)
    }
    async fn list(&self, _: PageRequest) -> Result<AccountPage, AccountError> {
        unimplemented!()
    }
}
fn request(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}
#[tokio::test]
async fn registration_is_pending_and_gates_password_backend_and_tokens() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let accounts = Arc::new(Accounts {
        account: Mutex::new(None),
        repo: repo.clone(),
    });
    let components = AuthComponentsFactory {
        repo: repo.clone(),
        accounts: accounts.clone(),
        ..Default::default()
    }
    .build();
    let backend = ExemplumAuthBackend::from_arc(repo.clone(), accounts.clone());
    let app = exemplum_dmn_auth::router(components).layer(
        axum_login::AuthManagerLayerBuilder::new(
            PrincipalAuthBackend::new(backend.clone()),
            tower_sessions::SessionManagerLayer::new(tower_sessions::MemoryStore::default()),
        )
        .build(),
    );
    let registration = serde_json::json!({"display_name":" Alice ","username":" Alice ","password":"long password!"});
    assert_eq!(
        app.clone()
            .oneshot(request("/register", registration.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.clone()
            .oneshot(request("/register", registration))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let identity = repo
        .find_active_by_type_and_identifier("password", "Alice")
        .await
        .unwrap()
        .unwrap();
    assert!(
        backend
            .resolve_principal(identity.id)
            .await
            .unwrap()
            .is_none()
    );
    let login = serde_json::json!({"username":" Alice ","password":"long password!"});
    assert_eq!(
        app.clone()
            .oneshot(request("/login", login.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let key = "secret-api-key";
    repo.create(
        uuid::Uuid::new_v4(),
        identity.user_id,
        "api_key",
        "client",
        &hypgm_authn::api_key::hash_api_key(key),
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let token = serde_json::json!({"client_id":"client","client_secret":key});
    assert_eq!(
        app.clone()
            .oneshot(request("/token", token.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    accounts
        .account
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .approval_status = ApprovalStatus::Approved;
    assert!(
        backend
            .resolve_principal(identity.id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        app.clone()
            .oneshot(request("/login", login))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.oneshot(request("/token", token))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn invalid_registration_and_role_injection_are_rejected() {
    let components = AuthComponentsFactory::default().build();
    for password in ["short".to_string(), "x".repeat(129), "界".repeat(129)] {
        let error = components
            .register(RegistrationRequest {
                display_name: "Alice".into(),
                username: "alice".into(),
                password,
            })
            .await
            .unwrap_err();
        assert_eq!(error.classification().status, StatusCode::BAD_REQUEST);
    }
    for field in [
        "is_admin",
        "approval_status",
        "welcome_scheduled_at",
        "welcome_processed_at",
    ] {
        let mut body = serde_json::json!({"display_name":"Alice","username":"alice","password":"long password!"});
        body[field] = serde_json::json!(true);
        assert!(serde_json::from_value::<RegistrationRequest>(body).is_err());
    }
}

#[tokio::test]
async fn bearer_claims_cannot_bypass_current_approval() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let accounts = Arc::new(Accounts {
        account: Mutex::new(None),
        repo: repo.clone(),
    });
    let components = AuthComponentsFactory {
        repo: repo.clone(),
        accounts: accounts.clone(),
        ..Default::default()
    }
    .build();
    let registered = components
        .register(RegistrationRequest {
            display_name: "Alice".into(),
            username: "alice".into(),
            password: "long password!".into(),
        })
        .await
        .unwrap();
    let identity = repo
        .find_active_by_type_and_identifier("password", "alice")
        .await
        .unwrap()
        .unwrap();
    let backend = ExemplumAuthBackend::from_arc(repo, accounts.clone());
    let secret = b"test-secret-longer-than-thirty-two-bytes";
    let claims = hypgm_authn::jwt::Claims::new(
        identity.id,
        registered.id,
        (chrono::Utc::now().timestamp() + 3600) as usize,
    );
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret),
    )
    .unwrap();
    for shared_validator in [false, true] {
        for html in [false, true] {
            accounts
                .account
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .approval_status = ApprovalStatus::Pending;
            let app = axum::Router::new().route(
                "/me",
                axum::routing::get(|_: hypgm_authn::extractor::RequireAuth| async { "ok" }),
            );
            let app = if html {
                app.layer(axum::middleware::from_fn_with_state(
                    backend.clone(),
                    exemplum_dmn_auth::backend::html_bearer_gate,
                ))
            } else {
                app.layer(axum::middleware::from_fn_with_state(
                    backend.clone(),
                    exemplum_dmn_auth::backend::bearer_gate,
                ))
            };
            let validator = hypgm_authn::jwt::JwtValidator::from_hs256_secret(secret).unwrap();
            let app = if shared_validator {
                app.layer(axum::Extension(Arc::new(validator)))
            } else {
                app.layer(axum::Extension(validator))
            };
            let req = || {
                Request::builder()
                    .uri("/me")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap()
            };
            let response = app.clone().oneshot(req()).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()["www-authenticate"], "Bearer");
            assert!(
                response.headers()["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with(if html {
                        "text/html"
                    } else {
                        "application/json"
                    })
            );
            accounts
                .account
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .approval_status = ApprovalStatus::Approved;
            assert_eq!(app.oneshot(req()).await.unwrap().status(), StatusCode::OK);
        }
    }
}

#[tokio::test]
async fn duplicate_browser_registration_retains_profile_but_not_password() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let accounts = Arc::new(Accounts {
        account: Mutex::new(None),
        repo: repo.clone(),
    });
    let components = AuthComponentsFactory {
        repo,
        accounts,
        ..Default::default()
    }
    .build();
    components
        .register(RegistrationRequest {
            display_name: "Alice".into(),
            username: "alice".into(),
            password: "long password!".into(),
        })
        .await
        .unwrap();
    let app = exemplum_dmn_auth::view::router(components)
        .layer(axum::middleware::from_fn(hypgm_view::html_error_middleware));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "display_name=Alice&username=alice&password=long+password%21",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("<form"));
    assert!(html.contains("value=\"Alice\""));
    assert!(html.contains("value=\"alice\""));
    assert!(!html.contains("long password!"));
}
