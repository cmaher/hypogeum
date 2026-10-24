#![allow(deprecated)]
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Router};
use hypgm_errors::ServiceError;
use hypgm_http::{
    AppJson, HealthCheck, HttpError, REQUEST_ID_HEADER, RequestId, not_found_handler,
};
use hypgm_logging::{LogFormat, create_test_subscriber};
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
        .route("/healthz", get(|| async { (Extension(HealthCheck), "ok") }))
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
        .route(
            "/error/logged-500",
            get(|| async {
                tracing::error!(handler_error = true, "handler failed with internal error");
                HttpError::from(ServiceError::InvalidResponse("boom")).into_response()
            }),
        )
        .fallback(not_found_handler)
        .layer(hypgm_http::trace_layer())
}

#[tokio::test]
async fn test_tracing_preserves_nested_route_templates() {
    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);
    let routes = Router::new().route("/widgets/{id}", get(|| async { "ok" }));
    let app = Router::new()
        .nest("/api", routes)
        .layer(hypgm_http::trace_layer());
    let request = Request::builder()
        .uri("/api/widgets/private-id?token=private-query")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let lines = writer.lines();
    assert_eq!(lines.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(event["fields"]["matched_path"], "/api/widgets/{id}");
    assert_eq!(event["span"]["matched_path"], "/api/widgets/{id}");
    assert!(!writer.output_string().contains("private-id"));
    assert!(!writer.output_string().contains("private-query"));
}

#[tokio::test]
async fn test_unmatched_route_never_uses_raw_uri_as_template() {
    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);
    let app = Router::new()
        .fallback(not_found_handler)
        .layer(hypgm_http::trace_layer());
    let request = Request::builder()
        .uri("/api/hello")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let lines = writer.lines();
    assert_eq!(lines.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(event["fields"]["matched_path"], "unmatched");
}

#[tokio::test]
async fn test_health_response_marker_controls_success_logging_on_any_route() {
    let (subscriber, writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);
    let app = Router::new()
        .route("/probe", get(|| async { (Extension(HealthCheck), "ok") }))
        .route(
            "/probe-failed",
            get(|| async { (Extension(HealthCheck), HttpError::internal_error()) }),
        )
        .layer(hypgm_http::trace_layer());

    for (path, status) in [
        ("/probe", StatusCode::OK),
        ("/probe-failed", StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let request = Request::builder().uri(path).body(Body::empty()).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
    }

    let lines = writer.lines();
    assert_eq!(lines.len(), 2);
    for (line, (path, level)) in lines
        .iter()
        .zip([("/probe", "DEBUG"), ("/probe-failed", "ERROR")])
    {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(event["level"], level);
        assert_eq!(event["fields"]["matched_path"], path);
    }
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

#[tokio::test]
async fn test_trace_layer_installation_and_execution() {
    use hypgm_http::TraceLayer;

    let (subscriber, _writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);

    // Verify both trace_layer() and TraceLayer::new() are installable and functional
    let app1 = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .layer(hypgm_http::trace_layer());

    let req1 = Request::builder()
        .method("GET")
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();

    let resp1 = app1.oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    assert!(resp1.headers().contains_key(REQUEST_ID_HEADER.clone()));

    let app2 = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .layer(TraceLayer::new());

    let req2 = Request::builder()
        .method("GET")
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();

    let resp2 = app2.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    assert!(resp2.headers().contains_key(REQUEST_ID_HEADER.clone()));
}

#[tokio::test]
async fn test_trace_layer_preserves_concurrency_limiter_readiness() {
    use std::convert::Infallible;
    use std::time::Duration;
    use tower::limit::ConcurrencyLimit;
    use tower::{Layer, Service};

    let (subscriber, _writer) = create_test_subscriber("debug", LogFormat::Json).unwrap();
    let _guard = tracing::subscriber::set_default(subscriber);

    let inner = tower::service_fn(|_: Request<Body>| async {
        Ok::<_, Infallible>(StatusCode::OK.into_response())
    });
    let mut service = hypgm_http::trace_layer().layer(ConcurrencyLimit::new(inner, 1));

    for _ in 0..2 {
        let ready = tokio::time::timeout(Duration::from_secs(1), service.ready())
            .await
            .expect("the completed request must release the concurrency permit")
            .unwrap();
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let response = ready.call(req).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(REQUEST_ID_HEADER.clone()));
    }
}

#[tokio::test]
async fn test_handler_error_retains_context_under_warn_and_error_filters() {
    let app = create_test_app();

    for filter_level in ["warn", "error"] {
        let (subscriber, writer) = create_test_subscriber(filter_level, LogFormat::Json).unwrap();
        let _guard = tracing::subscriber::set_default(subscriber);

        let req = Request::builder()
            .method("GET")
            .uri("/error/logged-500")
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let returned_id = resp
            .headers()
            .get(REQUEST_ID_HEADER.clone())
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let lines = writer.lines();
        assert_eq!(
            lines.len(),
            2,
            "filter '{}' should capture handler error and 500 completion event",
            filter_level
        );

        let mut found_handler_error = false;
        let mut found_completion = false;

        for line in lines {
            let val: serde_json::Value = serde_json::from_str(&line).unwrap();

            // Span association must be preserved even under warn/error filters
            let span = val.get("span").expect("event must have enclosing span");
            assert_eq!(
                span["request_id"], returned_id,
                "span request_id must match response request_id under filter '{}'",
                filter_level
            );
            assert_eq!(
                span["matched_path"], "/error/logged-500",
                "span matched_path must be present under filter '{}'",
                filter_level
            );

            if val["fields"].get("handler_error") == Some(&serde_json::Value::Bool(true)) {
                found_handler_error = true;
                assert_eq!(val["level"], "ERROR");
            }
            if val["fields"].get("message")
                == Some(&serde_json::Value::String("request completed".into()))
            {
                found_completion = true;
                assert_eq!(val["level"], "ERROR");
                assert_eq!(val["fields"]["status"], 500);
                assert_eq!(val["fields"]["request_id"], returned_id);
            }
        }

        assert!(
            found_handler_error,
            "handler error must be captured under filter '{}'",
            filter_level
        );
        assert!(
            found_completion,
            "completion error must be captured under filter '{}'",
            filter_level
        );
    }
}
