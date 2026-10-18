use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use exemplum_dmn_hello_world::HelloWorldError;
use exemplum_dmn_hello_world::controller::router;
use exemplum_dmn_hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use std::sync::Arc;
use tower::ServiceExt;

fn test_app(service: Arc<dyn HelloWorldService>) -> axum::Router {
    let (schema, policies) =
        hypgm_authz::load_schema_and_policies(CEDAR_SCHEMA, PERMIT_POLICY).unwrap();
    let authorizer = Arc::new(hypgm_authz::Authorizer::new(schema, policies));
    let user_service = Arc::new(MockUserService {
        user: exemplum_dmn_users::models::User {
            id: uuid::Uuid::new_v4(),
            display_name: "Test User".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    });
    router(service, authorizer, user_service)
}

#[tokio::test]
async fn test_say_hello_success_and_extra_fields_ignored() {
    let app = test_app(Arc::new(DefaultHelloWorldService));

    // Normal greeting
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"name":" Ada ","extra":123,"nested":{"a":true}}"#,
        ))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["message"], "Hello, Ada!");

    // Blank name defaults to world
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":""}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["message"], "Hello, world!");
}

#[tokio::test]
async fn test_service_validation_errors_mapped_to_safe_400() {
    let app = test_app(Arc::new(DefaultHelloWorldService));

    // NUL byte rejection in service
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"x\u0000y"}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "validation_error");
    assert_eq!(json["message"], "Validation failed");
    // Ensure raw error message ("Name must contain at most 200 characters without NUL") is not in response
    assert!(!String::from_utf8_lossy(&bytes).contains("without NUL"));

    // Name exceeding 200 Unicode scalars
    let long_name = "🦀".repeat(201);
    let body_str = format!(r#"{{"name":"{}"}}"#, long_name);
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body_str))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "validation_error");
    assert_eq!(json["message"], "Validation failed");
}

#[tokio::test]
async fn test_missing_required_name_field() {
    let app = test_app(Arc::new(DefaultHelloWorldService));

    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"not_name":"Ada"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "invalid_request");
    assert_eq!(json["message"], "Invalid request");
}

struct FailingService;

#[async_trait]
impl HelloWorldService for FailingService {
    async fn say_hello(&self, name: &str) -> Result<String, HelloWorldError> {
        if name == "not_found" {
            Err(HelloWorldError::NotFound)
        } else {
            Err(HelloWorldError::Internal("private internal secret".into()))
        }
    }
}

#[tokio::test]
async fn test_injected_service_delegation_and_error_propagation() {
    let app = test_app(Arc::new(FailingService));

    // Internal service error mapped to 500 without leaking private details
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"test"}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "internal_error");
    assert_eq!(json["message"], "Internal server error");
    assert!(!String::from_utf8_lossy(&bytes).contains("private internal secret"));

    // Repo NotFound mapped to 404
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"not_found"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "not_found");
    assert_eq!(json["message"], "Resource not found");
}

struct MockUserService {
    user: exemplum_dmn_users::models::User,
}

#[async_trait]
impl exemplum_dmn_users::service::UserService for MockUserService {
    async fn create(
        &self,
        _name: &str,
    ) -> Result<exemplum_dmn_users::models::User, exemplum_dmn_users::service::UserError> {
        unimplemented!()
    }
    async fn get(
        &self,
        id: uuid::Uuid,
    ) -> Result<exemplum_dmn_users::models::User, exemplum_dmn_users::service::UserError> {
        if id == self.user.id {
            Ok(self.user.clone())
        } else {
            Err(exemplum_dmn_users::service::UserError::NotFound)
        }
    }
    async fn list(
        &self,
        _page: exemplum_dmn_users::models::PageRequest,
    ) -> Result<exemplum_dmn_users::models::UserPage, exemplum_dmn_users::service::UserError> {
        unimplemented!()
    }
    async fn update_name(
        &self,
        _id: uuid::Uuid,
        _name: &str,
    ) -> Result<exemplum_dmn_users::models::User, exemplum_dmn_users::service::UserError> {
        unimplemented!()
    }
    async fn delete(&self, _id: uuid::Uuid) -> Result<(), exemplum_dmn_users::service::UserError> {
        unimplemented!()
    }
}

const CEDAR_SCHEMA: &str = r#"
entity User;
entity Resource;
action "greet" appliesTo {
    principal: [User],
    resource: [Resource]
};
"#;

const PERMIT_POLICY: &str = r#"
permit(
    principal,
    action == Action::"greet",
    resource == Resource::"hello_world"
);
"#;

const FORBID_POLICY: &str = r#"
forbid(
    principal,
    action == Action::"greet",
    resource == Resource::"hello_world"
);
"#;

#[tokio::test]
async fn test_hello_me_requires_auth() {
    let (schema, policies) =
        hypgm_authz::load_schema_and_policies(CEDAR_SCHEMA, PERMIT_POLICY).unwrap();
    let authorizer = Arc::new(hypgm_authz::Authorizer::new(schema, policies));
    let user_id = uuid::Uuid::new_v4();
    let user_service = Arc::new(MockUserService {
        user: exemplum_dmn_users::models::User {
            id: user_id,
            display_name: "Test User".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    });

    let app = exemplum_dmn_hello_world::controller::router(
        Arc::new(DefaultHelloWorldService),
        authorizer,
        user_service,
    );

    let req = Request::builder()
        .method("GET")
        .uri("/me")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_hello_me_evaluates_cedar_policy_and_returns_display_name() {
    let (schema, policies) =
        hypgm_authz::load_schema_and_policies(CEDAR_SCHEMA, PERMIT_POLICY).unwrap();
    let permit_authorizer = Arc::new(hypgm_authz::Authorizer::new(schema, policies));
    let user_id = uuid::Uuid::new_v4();
    let auth_id = uuid::Uuid::new_v4();
    let user_service = Arc::new(MockUserService {
        user: exemplum_dmn_users::models::User {
            id: user_id,
            display_name: "Alice Liddell".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    });

    let app = exemplum_dmn_hello_world::controller::router(
        Arc::new(DefaultHelloWorldService),
        permit_authorizer,
        user_service.clone(),
    );

    let principal = hypgm_authn::Principal::new(auth_id, user_id, hypgm_authn::AuthMethod::Session);
    let mut req = Request::builder()
        .method("GET")
        .uri("/me")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(principal.clone());

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["message"], "Hello, Alice Liddell!");

    // Test with forbidden policy -> 403
    let (forbid_schema, forbid_policies) =
        hypgm_authz::load_schema_and_policies(CEDAR_SCHEMA, FORBID_POLICY).unwrap();
    let forbid_authorizer = Arc::new(hypgm_authz::Authorizer::new(forbid_schema, forbid_policies));
    let app_forbidden = exemplum_dmn_hello_world::controller::router(
        Arc::new(DefaultHelloWorldService),
        forbid_authorizer,
        user_service,
    );

    let mut req_forbidden = Request::builder()
        .method("GET")
        .uri("/me")
        .body(Body::empty())
        .unwrap();
    req_forbidden.extensions_mut().insert(principal);

    let resp_forbidden = app_forbidden.oneshot(req_forbidden).await.unwrap();
    assert_eq!(resp_forbidden.status(), StatusCode::FORBIDDEN);
}
