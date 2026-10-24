use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use hypgm_errors::PublicErrorInfo;
use hypgm_http::{ErrorClassification, MAX_BODY_BYTES};
use hypgm_view::{AppForm, CROSS_SITE_FORM, HTML_CONTENT_TYPE};
use tower::ServiceExt;

const FORM: &str = "application/x-www-form-urlencoded";

#[derive(serde::Deserialize)]
struct GreetingForm {
    name: String,
}

fn router() -> Router {
    Router::new().route(
        "/form",
        post(|AppForm(form): AppForm<GreetingForm>| async move { form.name }),
    )
}

async fn send(content_type: Option<&str>, body: Body, length: Option<usize>) -> Response {
    let mut request = Request::builder().method("POST").uri("/form");
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(length) = length {
        request = request.header(header::CONTENT_LENGTH, length.to_string());
    }
    router().oneshot(request.body(body).unwrap()).await.unwrap()
}

async fn body_text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn assert_html_rejection(response: Response, expected: PublicErrorInfo) {
    assert_eq!(response.status(), expected.status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);
    assert_eq!(
        response.extensions().get::<ErrorClassification>(),
        Some(&expected)
    );
    let body = body_text(response).await;
    assert!(body.starts_with("<!DOCTYPE html>"));
    assert!(body.contains(expected.message));
}

fn padded_form(total_bytes: usize) -> Vec<u8> {
    let mut data = b"name=Ada&pad=".to_vec();
    assert!(total_bytes >= data.len());
    data.resize(total_bytes, b'x');
    data
}

#[tokio::test]
async fn valid_encoded_forms_decode() {
    for content_type in [
        FORM,
        "application/x-www-form-urlencoded; charset=UTF-8",
        "Application/X-WWW-Form-Urlencoded",
    ] {
        let response = send(
            Some(content_type),
            Body::from("name=Ada+Lovelace%20%3Cb%3E%F0%9F%A6%80"),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{content_type}");
        assert_eq!(body_text(response).await, "Ada Lovelace <b>🦀");
    }
}

#[tokio::test]
async fn unsupported_media_types_are_html_415() {
    for content_type in [
        None,
        Some("application/json"),
        Some("multipart/form-data; boundary=x"),
        Some("text/plain"),
    ] {
        let response = send(content_type, Body::from("name=Ada"), None).await;
        assert_html_rejection(response, PublicErrorInfo::UNSUPPORTED_MEDIA_TYPE).await;
    }
}

#[tokio::test]
async fn malformed_forms_are_html_400() {
    for body in [
        Body::from(""),
        Body::from("other=1"),
        Body::from("name=Ada&name=Grace"),
    ] {
        let response = send(Some(FORM), body, None).await;
        assert_html_rejection(response, PublicErrorInfo::INVALID_REQUEST).await;
    }
}

#[tokio::test]
async fn fixed_length_boundary() {
    let body = padded_form(MAX_BODY_BYTES);
    assert_eq!(body.len(), 16_384);
    let response = send(Some(FORM), Body::from(body.clone()), Some(body.len())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "Ada");

    let body = padded_form(MAX_BODY_BYTES + 1);
    let response = send(Some(FORM), Body::from(body.clone()), Some(body.len())).await;
    assert_html_rejection(response, PublicErrorInfo::PAYLOAD_TOO_LARGE).await;

    let response = send(Some(FORM), Body::from("name=Ada"), Some(MAX_BODY_BYTES + 1)).await;
    assert_html_rejection(response, PublicErrorInfo::PAYLOAD_TOO_LARGE).await;
}

async fn send_with_headers(headers: &[(&str, &str)]) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri("/form")
        .header(header::CONTENT_TYPE, FORM);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    router()
        .oneshot(request.body(Body::from("name=Ada")).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn same_origin_and_non_browser_submissions_are_accepted() {
    for headers in [
        &[][..],
        &[("sec-fetch-site", "same-origin")],
        &[("sec-fetch-site", "none")],
        &[
            ("origin", "http://app.test:8080"),
            ("host", "app.test:8080"),
        ],
        &[("origin", "https://APP.test"), ("host", "app.test")],
    ] {
        let response = send_with_headers(headers).await;
        assert_eq!(response.status(), StatusCode::OK, "{headers:?}");
        assert_eq!(body_text(response).await, "Ada");
    }
}

#[tokio::test]
async fn cross_site_submissions_are_html_403() {
    for headers in [
        &[("sec-fetch-site", "cross-site")][..],
        &[("sec-fetch-site", "same-site")],
        // Fetch metadata wins over a matching Origin.
        &[
            ("sec-fetch-site", "cross-site"),
            ("origin", "http://app.test"),
            ("host", "app.test"),
        ],
        &[("origin", "http://evil.test"), ("host", "app.test")],
        &[
            ("origin", "http://app.test:9090"),
            ("host", "app.test:8080"),
        ],
        &[("origin", "null"), ("host", "app.test")],
    ] {
        let response = send_with_headers(headers).await;
        assert_html_rejection(response, CROSS_SITE_FORM).await;
    }
}

fn streamed(total_bytes: usize) -> Body {
    let data = padded_form(total_bytes);
    let (first, second) = data.split_at(100);
    Body::from_stream(futures_util::stream::iter(vec![
        Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(first)),
        Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(second)),
    ]))
}

#[tokio::test]
async fn streamed_boundary_without_content_length() {
    let response = send(Some(FORM), streamed(MAX_BODY_BYTES), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "Ada");

    let response = send(Some(FORM), streamed(MAX_BODY_BYTES + 1), None).await;
    assert_html_rejection(response, PublicErrorInfo::PAYLOAD_TOO_LARGE).await;
}
