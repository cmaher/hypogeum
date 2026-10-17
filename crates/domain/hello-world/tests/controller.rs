use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use errors::{RepoError, ServiceError};
use hello_world::controller::router;
use hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn test_say_hello_success_and_extra_fields_ignored() {
    let app = router(Arc::new(DefaultHelloWorldService));

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
    let app = router(Arc::new(DefaultHelloWorldService));

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
    let app = router(Arc::new(DefaultHelloWorldService));

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
    async fn say_hello(&self, name: &str) -> Result<String, ServiceError> {
        if name == "not_found" {
            Err(ServiceError::Repo(RepoError::NotFound))
        } else {
            Err(ServiceError::InvalidResponse("private internal secret"))
        }
    }
}

#[tokio::test]
async fn test_injected_service_delegation_and_error_propagation() {
    let app = router(Arc::new(FailingService));

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
