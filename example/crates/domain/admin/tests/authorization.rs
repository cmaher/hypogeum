use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use exemplum_dmn_admin::{
    AdminComponents, AdminError, AdminService, DefaultAdminService, api, authorization::*,
};
use exemplum_dmn_users::{
    accounts::{Account, AccountError, AccountPage, AccountService, ApprovalStatus},
    models::PageRequest,
};
use hypgm_authn::{AuthMethod, Principal};
use hypgm_authz::{Authorizer, AuthzError};
use hypgm_errors::{PublicError, RepoError};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use uuid::Uuid;

struct Accounts(Mutex<Account>);
#[async_trait]
impl AccountService for Accounts {
    async fn register(&self, _: &str, _: &str, _: &str) -> Result<Account, AccountError> {
        unreachable!()
    }
    async fn get(&self, _: Uuid) -> Result<Account, AccountError> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn approve(&self, _: Uuid) -> Result<Account, AccountError> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn list(&self, _: PageRequest) -> Result<AccountPage, AccountError> {
        Ok(AccountPage {
            accounts: vec![self.0.lock().unwrap().clone()],
            next_after: None,
        })
    }
}
struct FailingAccounts(fn() -> AccountError);
#[async_trait]
impl AccountService for FailingAccounts {
    async fn register(&self, _: &str, _: &str, _: &str) -> Result<Account, AccountError> {
        unreachable!()
    }
    async fn get(&self, _: Uuid) -> Result<Account, AccountError> {
        Err((self.0)())
    }
    async fn approve(&self, _: Uuid) -> Result<Account, AccountError> {
        unreachable!()
    }
    async fn list(&self, _: PageRequest) -> Result<AccountPage, AccountError> {
        unreachable!()
    }
}
fn checked_in_authorizer() -> Arc<Authorizer> {
    let (schema, policies) = hypgm_authz::load_schema_and_policies(
        include_str!("../../../../config/schema.cedarschema"),
        include_str!("../../../../config/policies.cedar"),
    )
    .unwrap();
    Arc::new(Authorizer::new(schema, policies).unwrap())
}
fn setup() -> (AdminComponents, Arc<Accounts>, Principal) {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let accounts = Arc::new(Accounts(Mutex::new(Account {
        id,
        display_name: "Admin".into(),
        created_at: now,
        updated_at: now,
        approval_status: ApprovalStatus::Approved,
        is_admin: true,
        approved_at: Some(now),
        welcome_scheduled_at: None,
        welcome_processed_at: None,
    })));
    let service = DefaultAdminService::new(accounts.clone(), checked_in_authorizer());
    (
        AdminComponents::new(Arc::new(service)),
        accounts,
        Principal::new(Uuid::new_v4(), id, AuthMethod::Session),
    )
}
#[tokio::test]
async fn checked_in_policy_allows_admin_actions_and_reloads_authority() {
    let (components, accounts, principal) = setup();
    let service = components.service;
    assert_ne!(principal.auth_id, principal.user_id);
    assert!(
        service
            .list(&principal, PageRequest::default())
            .await
            .is_ok()
    );
    assert!(service.get(&principal, principal.user_id).await.is_ok());
    assert!(service.approve(&principal, principal.user_id).await.is_ok());
    accounts.0.lock().unwrap().is_admin = false;
    assert!(matches!(
        service.list(&principal, PageRequest::default()).await,
        Err(AdminError::Authz(AuthzError::Forbidden))
    ));
    assert!(
        service
            .approve(&principal, principal.user_id)
            .await
            .is_err()
    );
    accounts.0.lock().unwrap().is_admin = true;
    accounts.0.lock().unwrap().approval_status = ApprovalStatus::Pending;
    assert!(service.get(&principal, principal.user_id).await.is_err());
}
#[tokio::test]
async fn missing_accounts_are_forbidden_but_lookup_failures_are_internal() {
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::Session);
    let missing = DefaultAdminService::new(
        Arc::new(FailingAccounts(|| RepoError::NotFound.into())),
        checked_in_authorizer(),
    );
    assert!(matches!(
        missing.list(&principal, PageRequest::default()).await,
        Err(AdminError::Authz(AuthzError::Forbidden))
    ));
    let broken = DefaultAdminService::new(
        Arc::new(FailingAccounts(|| {
            RepoError::database(std::io::Error::other("connection reset")).into()
        })),
        checked_in_authorizer(),
    );
    let error = broken
        .approve(&principal, principal.user_id)
        .await
        .unwrap_err();
    assert!(matches!(error, AdminError::Account(_)));
    assert_eq!(
        error.public_error().status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
#[tokio::test]
async fn json_routes_authenticate_paginate_and_authorize_writes() {
    let (components, accounts, principal) = setup();
    let app = api::router(components);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/users")
                .extension(principal.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(data["accounts"][0]["display_name"], "Admin");
    assert!(data["accounts"][0].get("password_hash").is_none());
    for limit in [0, 101] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/users?limit={limit}"))
                    .extension(principal.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    // Cross-site write rejection is installed and tested at the composition root.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/users/{}/approve", principal.user_id))
                .extension(principal.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    accounts.0.lock().unwrap().is_admin = false;
    for (method, uri) in [
        ("GET", "/users".to_string()),
        ("POST", format!("/users/{}/approve", principal.user_id)),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .extension(principal.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
#[test]
fn missing_attributes_and_unknown_actions_deny() {
    let (_, accounts, principal) = setup();
    let authorizer = checked_in_authorizer();
    assert!(
        authorizer
            .authorize(
                &principal,
                (ACTION_TYPE, VIEW_ADMIN_USERS),
                (RESOURCE_TYPE, ADMIN_USERS)
            )
            .is_err()
    );
    assert!(
        authorize(
            &authorizer,
            &principal,
            &accounts.0.lock().unwrap(),
            "unknown"
        )
        .is_err()
    );
}

#[tokio::test]
async fn browser_rows_escape_poll_stop_and_enforce_current_authority() {
    let (components, accounts, principal) = setup();
    accounts.0.lock().unwrap().welcome_scheduled_at = Some(chrono::Utc::now());
    accounts.0.lock().unwrap().display_name = "<script>alert(1)</script>".into();
    let app = exemplum_dmn_admin::view::router(components)
        .layer(axum::middleware::from_fn(hypgm_view::cross_site_middleware));
    for path in ["/users".to_owned(), format!("/users/{}", principal.user_id)] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains("text/html")
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .extension(principal.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("hx-trigger=\"every 2s\""));
        assert!(html.contains("Awaiting processing"));
        assert!(!html.contains("<script>alert"));
        assert!(
            html.contains("&lt;script&gt;") || html.contains("&#60;script&#62;"),
            "{html}"
        );
    }
    let approve = format!("/users/{}/approve", principal.user_id);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&approve)
                .extension(principal.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    accounts.0.lock().unwrap().welcome_processed_at = Some(chrono::Utc::now());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&approve)
                .header("hx-request", "true")
                .extension(principal.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.starts_with("<tr"));
    assert!(!html.contains("hx-trigger"));
    assert!(html.contains("<time>"));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&approve)
                .header("sec-fetch-site", "cross-site")
                .extension(principal.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    accounts.0.lock().unwrap().is_admin = false;
    for (method, path) in [
        ("GET", "/users".to_owned()),
        ("GET", format!("/users/{}", principal.user_id)),
        ("POST", approve),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .extension(principal.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn unscheduled_accounts_do_not_poll_or_claim_processing() {
    for approved_at in [None, Some(chrono::Utc::now())] {
        let (components, accounts, principal) = setup();
        accounts.0.lock().unwrap().approved_at = approved_at;
        let app = exemplum_dmn_admin::view::router(components);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/users")
                    .extension(principal)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(
            !html.contains("hx-trigger"),
            "unscheduled account must not poll"
        );
        assert!(!html.contains("Awaiting processing"));
        assert!(html.contains("Not scheduled"));
    }
}

#[tokio::test]
async fn stale_welcome_work_stops_polling_without_claiming_completion() {
    let (components, accounts, principal) = setup();
    accounts.0.lock().unwrap().welcome_scheduled_at =
        Some(chrono::Utc::now() - chrono::TimeDelta::hours(1));
    let response = exemplum_dmn_admin::view::router(components)
        .oneshot(
            Request::builder()
                .uri(format!("/users/{}", principal.user_id))
                .extension(principal)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!html.contains("hx-trigger"), "{html}");
    assert!(html.contains("Not yet processed; refresh to check"));
    assert!(!html.contains("<time>"));
}
