use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::{Extension, Router};
use http_body_util::BodyExt;
use hypgm_authn::{AuthMethod, Principal, RequireAuth};
use hypgm_authz::{
    Authorizer, AuthzRejection, PermissionPolicy, RequirePermission, load_schema_and_policies,
    require_permission, require_permission_with_entities,
};
use tower::ServiceExt;
use uuid::Uuid;

const SCHEMA: &str = r#"
entity User;
entity Document;
action "View" appliesTo {
    principal: [User],
    resource: [Document],
    context: {
        auth_id: String,
        auth_method: String
    }
};
action "Delete" appliesTo {
    principal: [User],
    resource: [Document],
    context: {
        auth_id: String,
        auth_method: String
    }
};
"#;

const POLICIES: &str = r#"
// Allow View on Document::"allowed-doc" for BearerJwt
permit(
    principal,
    action == Action::"View",
    resource == Document::"allowed-doc"
) when {
    context.auth_method == "bearer_jwt"
};

// Explicitly forbid any user from Document::"secret-vault"
forbid(
    principal,
    action,
    resource == Document::"secret-vault"
);
"#;

struct ViewAllowedDocPolicy;
impl PermissionPolicy for ViewAllowedDocPolicy {
    fn action() -> &'static str {
        "Action::\"View\""
    }

    fn resource() -> &'static str {
        "Document::\"allowed-doc\""
    }
}

struct DeleteDocPolicy;
impl PermissionPolicy for DeleteDocPolicy {
    fn action() -> &'static str {
        "Action::\"Delete\""
    }

    fn resource() -> &'static str {
        "Document::\"allowed-doc\""
    }
}

async fn view_handler(_perm: RequirePermission<ViewAllowedDocPolicy>) -> impl IntoResponse {
    Json(serde_json::json!({ "result": "viewed" }))
}

async fn delete_handler(_perm: RequirePermission<DeleteDocPolicy>) -> impl IntoResponse {
    Json(serde_json::json!({ "result": "deleted" }))
}

async fn composed_handler(
    RequireAuth(principal): RequireAuth,
    _perm: RequirePermission<ViewAllowedDocPolicy>,
) -> impl IntoResponse {
    Json(serde_json::json!({ "result": "composed_ok", "user": principal.user_id.to_string() }))
}

async fn dynamic_handler(
    State(authorizer): State<Arc<Authorizer>>,
    Extension(principal): Extension<Principal>,
    Path(doc_id): Path<String>,
) -> Result<Json<serde_json::Value>, AuthzRejection> {
    require_permission(
        &authorizer,
        &principal,
        "Action::\"View\"",
        format!("Document::\"{doc_id}\""),
    )?;
    Ok(Json(
        serde_json::json!({ "result": "success", "doc": doc_id }),
    ))
}

fn build_test_app() -> (Router, Arc<Authorizer>) {
    let (schema, policies) = load_schema_and_policies(SCHEMA, POLICIES).unwrap();
    let authorizer = Arc::new(Authorizer::new(schema, policies));

    let app = Router::new()
        .route("/view", get(view_handler))
        .route("/delete", get(delete_handler))
        .route("/composed", get(composed_handler))
        .route("/doc/{doc_id}", get(dynamic_handler))
        .with_state(Arc::clone(&authorizer));

    (app, authorizer)
}

#[tokio::test]
async fn test_guard_permits_authorized_request() {
    let (app, _) = build_test_app();
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    let req = Request::builder()
        .uri("/view")
        .extension(principal)
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json["result"], "viewed");
}

#[tokio::test]
async fn test_guard_denies_unauthorized_request_with_403() {
    let (app, _) = build_test_app();
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    // Delete has no permit -> default deny
    let req = Request::builder()
        .uri("/delete")
        .extension(principal)
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json["code"], "forbidden");
    assert_eq!(json["message"], "Access denied by policy");

    // Ensure internal Cedar details / policies are NOT leaked
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(!body_str.contains("Action::\"Delete\""));
    assert!(!body_str.contains("Document::\"allowed-doc\""));
    assert!(!body_str.contains("cedar"));
}

#[tokio::test]
async fn test_guard_dynamic_require_permission_helper() {
    let (app, _) = build_test_app();
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    // 1. Allowed doc
    let req = Request::builder()
        .uri("/doc/allowed-doc")
        .extension(principal.clone())
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 2. Secret vault (explicit forbid)
    let req_forbidden = Request::builder()
        .uri("/doc/secret-vault")
        .extension(principal.clone())
        .body(Body::empty())
        .unwrap();

    let res_forbidden = app.clone().oneshot(req_forbidden).await.unwrap();
    assert_eq!(res_forbidden.status(), StatusCode::FORBIDDEN);

    let body_bytes = res_forbidden
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json["code"], "forbidden");
    assert_eq!(json["message"], "Access denied by policy");

    // 3. Unhandled doc (default deny)
    let req_unhandled = Request::builder()
        .uri("/doc/other-doc")
        .extension(principal)
        .body(Body::empty())
        .unwrap();

    let res_unhandled = app.oneshot(req_unhandled).await.unwrap();
    assert_eq!(res_unhandled.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_guard_unauthenticated_request_denied() {
    let (app, _) = build_test_app();

    // No credentials attached -> returns 401 Unauthorized
    let req = Request::builder().uri("/view").body(Body::empty()).unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json["code"], "unauthenticated");
    assert_eq!(json["message"], "Authentication required");
}

#[tokio::test]
async fn test_guard_composed_with_require_auth() {
    let (app, _) = build_test_app();
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    let req = Request::builder()
        .uri("/composed")
        .extension(principal.clone())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json["result"], "composed_ok");
    assert_eq!(json["user"], principal.user_id.to_string());
}

#[test]
fn test_guard_check_helper() {
    let (schema, policies) = load_schema_and_policies(SCHEMA, POLICIES).unwrap();
    let authorizer = Authorizer::new(schema, policies);
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    // ViewAllowedDocPolicy permits View on Document::"allowed-doc"
    assert!(RequirePermission::<ViewAllowedDocPolicy>::check(&authorizer, &principal).is_ok());

    // DeleteDocPolicy has no permit -> Forbidden
    let res = RequirePermission::<DeleteDocPolicy>::check(&authorizer, &principal);
    assert!(matches!(res, Err(AuthzRejection::Authz(_))));
}

#[test]
fn test_guard_dynamic_require_permission_with_entities() {
    let schema_str = r#"
    entity User;
    entity Document {
        is_secret: Bool
    };
    action "View" appliesTo {
        principal: [User],
        resource: [Document],
        context: {
            auth_id: String,
            auth_method: String
        }
    };
    "#;
    let policy_str = r#"
    permit(
        principal,
        action == Action::"View",
        resource
    ) when {
        resource.is_secret == false
    };
    "#;
    let (schema, policies) = load_schema_and_policies(schema_str, policy_str).unwrap();
    let authorizer = Authorizer::new(schema.clone(), policies);

    let entities_json = serde_json::json!([
        {
            "uid": { "type": "Document", "id": "doc-public" },
            "attrs": { "is_secret": false },
            "parents": []
        }
    ]);
    let entities = cedar_policy::Entities::from_json_value(entities_json, Some(&schema)).unwrap();
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    let res = require_permission_with_entities(
        &authorizer,
        &principal,
        "Action::\"View\"",
        "Document::\"doc-public\"",
        &entities,
    );
    assert!(res.is_ok());
}
