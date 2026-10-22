use askama::Template;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use hypgm_errors::{PublicError, PublicErrorInfo};
use hypgm_http::{ErrorClassification, ErrorCode, REQUEST_ID_HEADER, trace_layer};
use hypgm_view::{
    HTML_CONTENT_TYPE, HtmlError, HtmxRequest, ViewMode, append_vary_hx_request,
    html_error_middleware, html_error_response, html_method_not_allowed, html_not_found_handler,
    html_response, render_error_template, render_html_public_error, render_template,
};
use tower::ServiceExt;

async fn body_text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn mode_router() -> Router {
    Router::new().route(
        "/",
        get(|htmx: HtmxRequest| async move {
            match htmx.mode {
                ViewMode::FullPage => "full",
                ViewMode::Fragment => "fragment",
            }
        }),
    )
}

async fn mode_for(headers: &[(&str, &str)]) -> String {
    let mut request = Request::builder().uri("/");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = mode_router()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    body_text(response).await
}

#[tokio::test]
async fn mode_requires_exact_hx_request_true() {
    assert_eq!(mode_for(&[]).await, "full");
    assert_eq!(mode_for(&[("HX-Request", "true")]).await, "fragment");
    for value in ["True", "TRUE", "1", "false", "", " true"] {
        assert_eq!(mode_for(&[("HX-Request", value)]).await, "full", "{value:?}");
    }
}

#[tokio::test]
async fn history_restoration_always_selects_full_page() {
    assert_eq!(
        mode_for(&[
            ("HX-Request", "true"),
            ("HX-History-Restore-Request", "true")
        ])
        .await,
        "full"
    );
    assert_eq!(
        mode_for(&[("HX-History-Restore-Request", "true")]).await,
        "full"
    );
}

#[tokio::test]
async fn html_responses_set_content_type_and_vary() {
    let response = html_response(StatusCode::OK, "<p>ok</p>".into());
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);
    let vary: Vec<_> = response.headers().get_all(header::VARY).iter().collect();
    assert_eq!(vary, vec!["HX-Request"]);
    assert_eq!(body_text(response).await, "<p>ok</p>");
}

#[test]
fn vary_is_merged_without_replacing_existing_values() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    append_vary_hx_request(&mut headers);
    let vary: Vec<_> = headers.get_all(header::VARY).iter().collect();
    assert_eq!(vary, vec!["Accept-Encoding", "HX-Request"]);

    append_vary_hx_request(&mut headers);
    assert_eq!(headers.get_all(header::VARY).iter().count(), 2);

    let mut covered = axum::http::HeaderMap::new();
    covered.insert(header::VARY, HeaderValue::from_static("origin, hx-request"));
    append_vary_hx_request(&mut covered);
    assert_eq!(covered.get_all(header::VARY).iter().count(), 1);

    let mut wildcard = axum::http::HeaderMap::new();
    wildcard.insert(header::VARY, HeaderValue::from_static("*"));
    append_vary_hx_request(&mut wildcard);
    assert_eq!(wildcard.get_all(header::VARY).iter().count(), 1);
}

#[derive(Debug, thiserror::Error)]
#[error("private failure: SELECT secret FROM credentials")]
struct LeakyError;

impl PublicError for LeakyError {
    fn public_error(&self) -> PublicErrorInfo {
        PublicErrorInfo::INTERNAL_ERROR
    }
}

fn assert_classified(response: &Response, expected: PublicErrorInfo) {
    assert_eq!(response.status(), expected.status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);
    assert_eq!(
        response.extensions().get::<ErrorClassification>(),
        Some(&expected)
    );
    assert_eq!(
        response.extensions().get::<ErrorCode>(),
        Some(&ErrorCode(expected.code))
    );
    assert_eq!(
        response.extensions().get::<StatusCode>(),
        Some(&expected.status)
    );
}

#[tokio::test]
async fn html_errors_retain_classification_and_hide_internal_details() {
    let response = html_error_response(PublicErrorInfo::VALIDATION_ERROR);
    assert_classified(&response, PublicErrorInfo::VALIDATION_ERROR);
    let body = body_text(response).await;
    assert!(body.contains("Validation failed"));
    assert!(body.contains("data-view-error"));

    for response in [
        render_html_public_error(&LeakyError),
        HtmlError::from(LeakyError).into_response(),
    ] {
        assert_classified(&response, PublicErrorInfo::INTERNAL_ERROR);
        let body = body_text(response).await;
        assert!(body.contains("Internal server error"));
        assert!(!body.contains("private"));
        assert!(!body.contains("SELECT"));
    }
}

#[tokio::test]
async fn html_error_messages_are_escaped() {
    let response = html_error_response(PublicErrorInfo::validation_error("<b>bad</b> & \"x\""));
    let body = body_text(response).await;
    assert!(body.contains("&lt;b&gt;bad&lt;/b&gt; &amp; &quot;x&quot;"));
    assert!(!body.contains("<b>"));
}

#[tokio::test]
async fn html_method_not_allowed_preserves_allow() {
    let response = html_method_not_allowed("GET, POST");
    assert_classified(&response, PublicErrorInfo::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()[header::ALLOW], "GET, POST");

    let response = HtmlError::MethodNotAllowed {
        allow: "GET".into(),
    }
    .into_response();
    assert_eq!(response.headers()[header::ALLOW], "GET");
}

fn page_router() -> Router {
    Router::new()
        .route("/page", get(|| async { html_response(StatusCode::OK, "ok".into()) }))
        .fallback(html_not_found_handler)
        .layer(axum::middleware::from_fn(html_error_middleware))
        .layer(trace_layer())
}

#[tokio::test]
async fn routing_errors_render_html_through_trace_layer() {
    let response = page_router()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/page")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_classified(&response, PublicErrorInfo::METHOD_NOT_ALLOWED);
    let allow = response.headers()[header::ALLOW].to_str().unwrap().to_owned();
    assert!(allow.contains("GET"), "{allow}");
    assert!(!allow.contains("DELETE"), "{allow}");
    assert!(response.headers().contains_key(&REQUEST_ID_HEADER));
    assert!(body_text(response).await.contains("Method not allowed"));

    let response = page_router()
        .oneshot(Request::builder().uri("/missing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_classified(&response, PublicErrorInfo::NOT_FOUND);
    assert!(response.headers().contains_key(&REQUEST_ID_HEADER));
    assert!(body_text(response).await.contains("Resource not found"));
}

#[derive(Template)]
#[template(source = "<p title=\"{{ value }}\">{{ value }}</p>", ext = "html")]
struct EchoTemplate<'a> {
    value: &'a str,
}

#[tokio::test]
async fn templates_escape_user_input() {
    let template = EchoTemplate {
        value: "<script>alert(\"x\")</script>' &",
    };
    let response = render_template(StatusCode::OK, &template);
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);
    let body = body_text(response).await;
    assert!(!body.contains("<script>"));
    assert!(!body.contains("\"x\""));
    assert!(body.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn error_templates_keep_status_and_classification() {
    let template = EchoTemplate { value: "kept" };
    let response = render_error_template(PublicErrorInfo::VALIDATION_ERROR, &template);
    assert_classified(&response, PublicErrorInfo::VALIDATION_ERROR);
    assert!(body_text(response).await.contains("kept"));
}

struct FailingDisplay;

impl std::fmt::Display for FailingDisplay {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Err(std::fmt::Error)
    }
}

#[derive(Template)]
#[template(source = "<p>private template content {{ value }}</p>", ext = "html")]
struct FailingTemplate {
    value: FailingDisplay,
}

#[tokio::test]
async fn render_failures_return_safe_html_500() {
    for response in [
        render_template(
            StatusCode::OK,
            &FailingTemplate {
                value: FailingDisplay,
            },
        ),
        render_error_template(
            PublicErrorInfo::VALIDATION_ERROR,
            &FailingTemplate {
                value: FailingDisplay,
            },
        ),
    ] {
        assert_classified(&response, PublicErrorInfo::INTERNAL_ERROR);
        let body = body_text(response).await;
        assert!(body.contains("Internal server error"));
        assert!(!body.contains("private"));
    }
}
