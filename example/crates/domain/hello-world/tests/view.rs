use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use exemplum_dmn_hello_world::HelloWorldError;
use exemplum_dmn_hello_world::controller::HelloComponentsFactory;
use exemplum_dmn_hello_world::service::{
    DefaultHelloWorldService, HelloWorldService, NAME_VALIDATION_MESSAGE,
};
use exemplum_dmn_hello_world::view::router;
use hypgm_errors::PublicErrorInfo;
use hypgm_http::ErrorClassification;
use hypgm_view::{HTML_CONTENT_TYPE, HTMX_SCRIPT_URL, VIEW_SCRIPT_URL};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const FORM: &str = "application/x-www-form-urlencoded";

fn app(hello_service: Arc<dyn HelloWorldService>) -> axum::Router {
    router(
        HelloComponentsFactory {
            hello_service,
            ..Default::default()
        }
        .build(),
    )
}

fn default_app() -> axum::Router {
    app(Arc::new(DefaultHelloWorldService))
}

async fn get_page(app: axum::Router) -> Response {
    app.oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn post_form(app: axum::Router, body: &str, htmx: bool) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri("/hello")
        .header(header::CONTENT_TYPE, FORM);
    if htmx {
        request = request.header("HX-Request", "true");
    }
    app.oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn body_text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn assert_html(response: &Response, status: StatusCode) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);
    let vary: Vec<_> = response.headers().get_all(header::VARY).iter().collect();
    assert_eq!(vary, ["HX-Request", "HX-History-Restore-Request"]);
}

#[tokio::test]
async fn page_renders_plain_form_enhanced_with_htmx_and_shared_assets() {
    let response = get_page(default_app()).await;
    assert_html(&response, StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.starts_with("<!DOCTYPE html>"));
    assert!(body.contains(r#"method="post" action="/hello""#));
    assert!(body.contains(r#"hx-post="/hello""#));
    assert!(body.contains(r#"hx-target="closest [data-view-section]""#));
    assert!(body.contains("data-view-section"));
    assert!(body.contains("data-view-feedback"));
    assert!(body.contains(&format!(r#"<script src="{HTMX_SCRIPT_URL}" defer>"#)));
    assert!(body.contains(&format!(r#"<script src="{VIEW_SCRIPT_URL}" defer>"#)));
    assert!(!body.contains("hx-boost"));
    assert!(!body.contains("hx-push-url"));
    assert!(!body.contains("data-greeting"));
}

#[tokio::test]
async fn ordinary_submission_renders_full_page_with_greeting() {
    let response = post_form(default_app(), "name=+Ada+", false).await;
    assert_html(&response, StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.starts_with("<!DOCTYPE html>"));
    assert!(body.contains("Hello, Ada!"));
    assert!(body.contains(r#"value=" Ada ""#));
}

#[tokio::test]
async fn htmx_submission_renders_shared_section_fragment() {
    let fragment = body_text(post_form(default_app(), "name=Ada", true).await).await;
    assert!(fragment.starts_with("<section"));
    assert!(!fragment.contains("<html"));
    assert!(fragment.contains("Hello, Ada!"));

    let page = body_text(post_form(default_app(), "name=Ada", false).await).await;
    assert!(page.contains(fragment.trim()));
}

#[tokio::test]
async fn blank_and_missing_names_greet_world() {
    for body in ["name=", "name=+%09+", ""] {
        let response = post_form(default_app(), body, true).await;
        assert_html(&response, StatusCode::OK);
        assert!(
            body_text(response).await.contains("Hello, world!"),
            "{body:?}"
        );
    }
}

#[tokio::test]
async fn validation_keeps_status_safe_message_and_escaped_value() {
    let long_name = "<b>".repeat(70);
    let encoded = "%3Cb%3E".repeat(70);
    for htmx in [false, true] {
        let response = post_form(default_app(), &format!("name={encoded}"), htmx).await;
        assert_html(&response, StatusCode::BAD_REQUEST);
        assert_eq!(
            response.extensions().get::<ErrorClassification>(),
            Some(&PublicErrorInfo::VALIDATION_ERROR)
        );
        let body = body_text(response).await;
        assert!(body.contains(NAME_VALIDATION_MESSAGE));
        assert!(body.contains(r#"aria-invalid="true""#));
        assert!(body.contains(&"&#60;b&#62;".repeat(70)) || body.contains(&"&lt;b&gt;".repeat(70)));
        assert!(!body.contains(&long_name));
        assert_eq!(body.contains("<html"), !htmx);
    }

    let response = post_form(default_app(), "name=x%00y", true).await;
    assert_html(&response, StatusCode::BAD_REQUEST);
    let body = body_text(response).await;
    assert!(body.contains(NAME_VALIDATION_MESSAGE));
    assert!(!body.contains('\0'));
}

#[tokio::test]
async fn user_input_is_escaped() {
    let response = post_form(
        default_app(),
        "name=%3Cscript%3Ealert(%22x%22)%3C%2Fscript%3E",
        true,
    )
    .await;
    assert_html(&response, StatusCode::OK);
    let body = body_text(response).await;
    assert!(!body.contains("<script>alert"));
    assert!(!body.contains("\"x\""));
}

struct RecordingService {
    names: Mutex<Vec<String>>,
}

#[async_trait]
impl HelloWorldService for RecordingService {
    async fn say_hello(&self, name: &str) -> Result<String, HelloWorldError> {
        self.names.lock().unwrap().push(name.to_owned());
        Ok("Greetings from the injected service".into())
    }
}

#[tokio::test]
async fn submissions_invoke_injected_service_directly() {
    let service = Arc::new(RecordingService {
        names: Mutex::new(Vec::new()),
    });
    let response = post_form(app(service.clone()), "name=+Grace+", true).await;
    assert_html(&response, StatusCode::OK);
    assert!(
        body_text(response)
            .await
            .contains("Greetings from the injected service")
    );
    assert_eq!(*service.names.lock().unwrap(), [" Grace ".to_owned()]);
}

struct RejectingService;

#[async_trait]
impl HelloWorldService for RejectingService {
    async fn say_hello(&self, _name: &str) -> Result<String, HelloWorldError> {
        Err(HelloWorldError::Validation("That name is reserved."))
    }
}

#[tokio::test]
async fn validation_shows_the_service_message() {
    let response = post_form(app(Arc::new(RejectingService)), "name=admin", true).await;
    assert_html(&response, StatusCode::BAD_REQUEST);
    let body = body_text(response).await;
    assert!(body.contains("That name is reserved."), "{body}");
    assert!(!body.contains(NAME_VALIDATION_MESSAGE));
}

struct FailingService;

#[async_trait]
impl HelloWorldService for FailingService {
    async fn say_hello(&self, _name: &str) -> Result<String, HelloWorldError> {
        Err(HelloWorldError::Internal(
            "private connection detail postgres://secret".into(),
        ))
    }
}

#[tokio::test]
async fn internal_errors_render_safe_html() {
    for htmx in [false, true] {
        let response = post_form(app(Arc::new(FailingService)), "name=Ada", htmx).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);
        let body = body_text(response).await;
        assert!(body.contains("Internal server error"));
        assert!(body.contains("data-view-error"));
        assert!(!body.contains("private"));
        assert!(!body.contains("secret"));
    }
}

#[tokio::test]
async fn malformed_submissions_are_safe_html_errors() {
    let response = default_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/hello")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"Ada"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);

    let response = post_form(default_app(), "name=a&name=b", false).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.headers()[header::CONTENT_TYPE], HTML_CONTENT_TYPE);
}
