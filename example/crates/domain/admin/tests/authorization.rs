use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use exemplum_dmn_admin::{AdminService, api, authorization::*};
use exemplum_dmn_users::{
    accounts::{Account, AccountError, AccountPage, AccountService, ApprovalStatus},
    models::PageRequest,
};
use hypgm_authn::{AuthMethod, Principal};
use hypgm_authz::Authorizer;
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
fn setup() -> (AdminService, Arc<Accounts>, Principal) {
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
        welcome_processed_at: None,
    })));
    let (schema, policies) = hypgm_authz::load_schema_and_policies(
        include_str!("../../../../config/schema.cedarschema"),
        include_str!("../../../../config/policies.cedar"),
    )
    .unwrap();
    let service = AdminService::new(
        accounts.clone(),
        Arc::new(Authorizer::new(schema, policies).unwrap()),
    );
    (
        service,
        accounts,
        Principal::new(Uuid::new_v4(), id, AuthMethod::Session),
    )
}
#[tokio::test]
async fn checked_in_policy_allows_admin_actions_and_reloads_authority() {
    let (service, accounts, principal) = setup();
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
    assert_eq!(
        service
            .list(&principal, PageRequest::default())
            .await
            .unwrap_err()
            .classification()
            .status,
        StatusCode::FORBIDDEN
    );
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
async fn json_routes_authenticate_paginate_and_protect_writes() {
    let (service, accounts, principal) = setup();
    let app = api::router(service);
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
    for cross_site in [false, true] {
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/users/{}/approve", principal.user_id))
            .extension(principal.clone());
        if cross_site {
            req = req.header("sec-fetch-site", "cross-site");
        }
        let response = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if cross_site {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
    }
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
    let (schema, policies) = hypgm_authz::load_schema_and_policies(
        include_str!("../../../../config/schema.cedarschema"),
        include_str!("../../../../config/policies.cedar"),
    )
    .unwrap();
    let authorizer = Authorizer::new(schema, policies).unwrap();
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
