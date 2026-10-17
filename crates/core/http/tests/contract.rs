use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use errors::{RepoError, ServiceError};
use http_support::{
    AppJson, ErrorClassification, HttpError, MAX_BODY_BYTES, render_method_not_allowed,
};
use tower::ServiceExt;

#[tokio::test]
async fn test_response_serialization_failure_is_a_redacted_json_error() {
    struct FailingSerialization;

    impl serde::Serialize for FailingSerialization {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("private serialization detail"))
        }
    }

    let response = AppJson(FailingSerialization).into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        response.extensions().get::<ErrorClassification>(),
        Some(&ErrorClassification::INTERNAL_ERROR)
    );
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"code": "internal_error", "message": "Internal server error"})
    );
}

#[tokio::test]
async fn test_error_mappings_and_no_leakage() {
    let test_cases: Vec<(HttpError, StatusCode, &'static str, &'static str)> = vec![
        (
            HttpError::invalid_request(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid request",
        ),
        (
            HttpError::from(ServiceError::Validation("secret_user_input".to_string())),
            StatusCode::BAD_REQUEST,
            "validation_error",
            "Validation failed",
        ),
        (
            HttpError::from(RepoError::InvalidInput("private_query_details".to_string())),
            StatusCode::BAD_REQUEST,
            "validation_error",
            "Validation failed",
        ),
        (
            HttpError::from(RepoError::NotFound),
            StatusCode::NOT_FOUND,
            "not_found",
            "Resource not found",
        ),
        (
            HttpError::not_found(),
            StatusCode::NOT_FOUND,
            "not_found",
            "Resource not found",
        ),
        (
            HttpError::method_not_allowed("GET, POST"),
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "Method not allowed",
        ),
        (
            HttpError::payload_too_large(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Payload too large",
        ),
        (
            HttpError::unsupported_media_type(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Unsupported media type",
        ),
        (
            HttpError::from(RepoError::database(std::io::Error::other(
                "SELECT * FROM secrets failed",
            ))),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Internal server error",
        ),
        (
            HttpError::from(ServiceError::Remote(Box::new(std::io::Error::other(
                "connection to backend failed",
            )))),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Internal server error",
        ),
        (
            HttpError::from(ServiceError::InvalidResponse("malformed remote body")),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Internal server error",
        ),
    ];

    for (err, expected_status, expected_code, expected_message) in test_cases {
        let resp = err.into_response();
        assert_eq!(resp.status(), expected_status);

        // Classification extension
        let classification = resp
            .extensions()
            .get::<ErrorClassification>()
            .expect("Classification must be attached to response extensions");
        assert_eq!(classification.status, expected_status);
        assert_eq!(classification.code, expected_code);
        assert_eq!(classification.message, expected_message);

        // JSON body
        let body_bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();

        assert_eq!(json["code"], expected_code);
        assert_eq!(json["message"], expected_message);

        // Ensure no internal details leaked
        assert!(!body_str.contains("secret"));
        assert!(!body_str.contains("private"));
        assert!(!body_str.contains("SELECT"));
        assert!(!body_str.contains("backend"));
    }
}

#[tokio::test]
async fn test_method_not_allowed_preserves_allow_header() {
    let resp = render_method_not_allowed("GET, POST");
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(resp.headers().get(header::ALLOW).unwrap(), &"GET, POST");
    let classification = resp.extensions().get::<ErrorClassification>().unwrap();
    assert_eq!(classification.code, "method_not_allowed");
    assert_eq!(classification.message, "Method not allowed");

    // Verify typed HttpError::method_not_allowed also renders Allow header
    let typed_resp = HttpError::method_not_allowed("GET, POST").into_response();
    assert_eq!(typed_resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        typed_resp.headers().get(header::ALLOW).unwrap(),
        &"GET, POST"
    );
    let typed_classification = typed_resp
        .extensions()
        .get::<ErrorClassification>()
        .unwrap();
    assert_eq!(typed_classification.code, "method_not_allowed");
}

#[tokio::test]
async fn test_contextual_reclassification() {
    let err = HttpError::from(RepoError::NotFound);
    assert_eq!(err.classification().code, "not_found");

    let reclassified = err.reclassify(ErrorClassification::INVALID_REQUEST);
    assert_eq!(reclassified.classification().code, "invalid_request");
    assert_eq!(
        reclassified.classification().status,
        StatusCode::BAD_REQUEST
    );
}

#[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq)]
struct TestPayload {
    message: String,
    count: Option<u32>,
}

fn test_router() -> Router {
    Router::new().route(
        "/echo",
        post(|AppJson(payload): AppJson<TestPayload>| async move { AppJson(payload) }),
    )
}

#[tokio::test]
async fn test_json_extractor_content_type_validation() {
    let app = test_router();

    // 1. Missing Content-Type
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // 2. Unsupported Content-Type (text/plain)
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // 3. Valid application/json
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4. Valid application/json with parameters (charset)
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 5. Valid application/*+json suffix
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/problem+json")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 6. Malformed media type with extra slashes (e.g. application/not/a/type+json)
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/not/a/type+json")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // 7. Malformed media type with empty subtype prefix (e.g. application/+json)
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/+json")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // 8. Malformed media type with whitespace
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/bad space+json")
        .body(Body::from(r#"{"message":"hello"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn test_json_extractor_malformed_empty_wrong_type() {
    let app = test_router();

    // 1. Empty body
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "invalid_request");

    // 2. Malformed JSON
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"message": "incomplete"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "invalid_request");

    // 3. Shape/Type invalid (expected string message, got integer)
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"message": 12345}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "invalid_request");
}

fn make_padded_json(total_bytes: usize) -> Vec<u8> {
    let prefix = b"{\"message\":\"hello\"}";
    assert!(total_bytes >= prefix.len());
    let mut data = prefix.to_vec();
    data.resize(total_bytes, b' ');
    data
}

#[tokio::test]
async fn test_exact_16384_bytes_boundary_with_content_length() {
    let app = test_router();

    // Exactly 16,384 bytes with Content-Length succeeds
    let valid_body = make_padded_json(MAX_BODY_BYTES);
    assert_eq!(valid_body.len(), 16_384);
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, valid_body.len().to_string())
        .body(Body::from(valid_body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 16,385 bytes with Content-Length fails with 413
    let large_body = make_padded_json(MAX_BODY_BYTES + 1);
    assert_eq!(large_body.len(), 16_385);
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, large_body.len().to_string())
        .body(Body::from(large_body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "payload_too_large");
    assert_eq!(json["message"], "Payload too large");
}

#[tokio::test]
async fn test_exact_16384_bytes_boundary_streamed_without_content_length() {
    let app = test_router();

    // Streamed exactly 16,384 bytes without Content-Length succeeds
    let chunk1 = vec![b' '; 16_300];
    let mut chunk2 = b"{\"message\":\"hello\"}".to_vec();
    chunk2.resize(16_384 - 16_300, b' ');

    let stream = futures_util::stream::iter(vec![
        Ok::<_, std::io::Error>(bytes::Bytes::from(chunk2)),
        Ok::<_, std::io::Error>(bytes::Bytes::from(chunk1)),
    ]);
    let body = Body::from_stream(stream);
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Streamed 16,385 bytes without Content-Length fails with 413
    let chunk1 = vec![b' '; 16_300];
    let mut chunk2 = b"{\"message\":\"hello\"}".to_vec();
    chunk2.resize(16_385 - 16_300, b' ');

    let stream = futures_util::stream::iter(vec![
        Ok::<_, std::io::Error>(bytes::Bytes::from(chunk2)),
        Ok::<_, std::io::Error>(bytes::Bytes::from(chunk1)),
    ]);
    let body = Body::from_stream(stream);
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "payload_too_large");
}
