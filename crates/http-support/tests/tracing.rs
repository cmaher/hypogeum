use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::middleware::from_fn;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use errors::ServiceError;
use http_support::{
    AppJson, HttpError, REQUEST_ID_HEADER, RequestId, not_found_handler, trace_middleware,
};
use logging::{LogFormat, create_test_subscriber};
use tower::ServiceExt;
use tracing::Instrument;

#[derive(serde::Deserialize, serde::Serialize, Debug)]
struct DummyRequest {
    name: String,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
struct DummyResponse {
    message: String,
}

fn create_test_app() -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/api/hello",
            post(|AppJson(req): AppJson<DummyRequest>| async move {
                // Emit an event inside the handler to test context inheritance
                tracing::info!(service_event = true, "processing hello request");

                // Spawned task inheriting current span explicitly
                let span = tracing::Span::current();
                tokio::spawn(
                    async move {
                        tracing::info!(spawned_event = true, "background task completed");
                    }
                    .instrument(span),
                );

                AppJson(DummyResponse {
                    message: format!("Hello, {}!", req.name),
                })
            }),
        )
        .route(
            "/error/500",
            get(|| async {
                HttpError::from(ServiceError::InvalidResponse("boom")).into_response()
            }),
        )
        .fallback(not_found_handler)
        .layer(from_fn(trace_middleware))
}

#[tokio::test]
async fn test_request_id_overrides_caller_and_returns_on_response() {
    let app = create_test_app();

    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);

    let untrusted_id = "untrusted-caller-id-12345";
    let req = Request::builder()
        .method("POST")
        .uri("/api/hello")
        .header(header::CONTENT_TYPE, "application/json")
        .header(REQUEST_ID_HEADER.clone(), untrusted_id)
        .body(Body::from(r#"{"name":"Ada"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let returned_id = resp
        .headers()
        .get(REQUEST_ID_HEADER.clone())
        .expect("x-request-id header must be present on response")
        .to_str()
        .unwrap();

    // Must NOT be the caller supplied untrusted ID
    assert_ne!(returned_id, untrusted_id);
    // Must be a valid UUID v4
    let parsed_uuid = uuid::Uuid::parse_str(returned_id).expect("must be valid uuid");
    assert_eq!(parsed_uuid.get_version_num(), 4);

    // Extension must match
    let ext_id = resp.extensions().get::<RequestId>().unwrap();
    assert_eq!(ext_id.0, returned_id);

    // Check captured logs
    let output = writer.output_string();
    assert!(!output.contains(untrusted_id));
    assert!(output.contains(returned_id));
}

#[tokio::test]
async fn test_health_check_completion_is_debug() {
    let app = create_test_app();

    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);

    let req = Request::builder()
        .method("GET")
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let lines = writer.lines();
    assert_eq!(lines.len(), 1, "exactly one completion event for healthz");

    let val: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(val["level"], "DEBUG");
    let fields = &val["fields"];
    assert_eq!(fields["message"], "request completed");
    assert_eq!(fields["status"], 200);
    assert_eq!(fields["matched_path"], "/healthz");
    assert_eq!(fields["method"], "GET");
    assert!(fields["elapsed_ms"].is_number());
}

#[tokio::test]
async fn test_5xx_error_completion_is_error_level() {
    let app = create_test_app();

    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);

    let req = Request::builder()
        .method("GET")
        .uri("/error/500")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let lines = writer.lines();
    assert_eq!(lines.len(), 1, "exactly one completion event for 500");

    let val: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(val["level"], "ERROR");
    let fields = &val["fields"];
    assert_eq!(fields["message"], "request completed");
    assert_eq!(fields["status"], 500);
    assert_eq!(fields["error_code"], "internal_error");
}

#[tokio::test]
async fn test_unmatched_route_is_info_and_unmatched_marker() {
    let app = create_test_app();

    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);

    let req = Request::builder()
        .method("GET")
        .uri("/unknown/endpoint")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let lines = writer.lines();
    assert_eq!(lines.len(), 1);

    let val: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(val["level"], "INFO");
    let fields = &val["fields"];
    assert_eq!(fields["status"], 404);
    assert_eq!(fields["matched_path"], "unmatched");
    assert_eq!(fields["error_code"], "not_found");
    // Ensure raw URI path is not leaked into fields
    assert_ne!(fields["matched_path"], "/unknown/endpoint");
}

#[tokio::test]
async fn test_context_inheritance_in_service_and_spawned_task() {
    let app = create_test_app();

    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);

    let req = Request::builder()
        .method("POST")
        .uri("/api/hello")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"Ada"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp
        .headers()
        .get(REQUEST_ID_HEADER.clone())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Give the spawned task a few milliseconds to complete
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let lines = writer.lines();
    // Lines should contain:
    // 1. service_event
    // 2. completion event
    // 3. spawned_event
    let mut found_service = false;
    let mut found_spawned = false;
    let mut found_completion = false;

    for line in lines {
        let val: serde_json::Value = serde_json::from_str(&line).unwrap();
        // Check span association: "span" field or top-level span must match request_id
        if let Some(span) = val.get("span") {
            assert_eq!(span["request_id"], req_id);
        }
        if val["fields"].get("service_event") == Some(&serde_json::Value::Bool(true)) {
            found_service = true;
        }
        if val["fields"].get("spawned_event") == Some(&serde_json::Value::Bool(true)) {
            found_spawned = true;
        }
        if val["fields"].get("message")
            == Some(&serde_json::Value::String("request completed".into()))
        {
            found_completion = true;
            assert_eq!(val["level"], "INFO");
            assert_eq!(val["fields"]["status"], 200);
            assert_eq!(val["fields"]["matched_path"], "/api/hello");
            assert_eq!(val["fields"]["request_id"], req_id);
        }
    }

    assert!(found_service, "service event must be recorded in log");
    assert!(found_spawned, "spawned task event must be recorded in log");
    assert!(found_completion, "completion event must be recorded in log");
}
