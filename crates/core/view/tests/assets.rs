use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use hypgm_view::assets::{self, ASSETS_MOUNT};
use hypgm_view::{HTMX_SCRIPT_URL, HTMX_VERSION, VIEW_SCRIPT_URL};
use tower::ServiceExt;

fn app() -> Router {
    Router::new().nest(ASSETS_MOUNT, assets::router())
}

async fn send(method: &str, uri: &str) -> Response {
    app()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn htmx_is_served_with_javascript_media_type_and_pinned_version() {
    assert!(HTMX_SCRIPT_URL.contains(HTMX_VERSION));
    let response = send("GET", HTMX_SCRIPT_URL).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    let body = body_text(response).await;
    assert!(body.contains(&format!("version:\"{HTMX_VERSION}\"")));
}

#[test]
fn vendored_license_records_version() {
    let license = include_str!("../assets/LICENSE.htmx");
    assert!(license.starts_with(&format!("htmx.org {HTMX_VERSION}\n")));
    assert!(license.contains("Zero-Clause BSD"));
}

#[tokio::test]
async fn view_configuration_is_served() {
    let response = send("GET", VIEW_SCRIPT_URL).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    // Behaviour is exercised in a browser by `example/tests/browser`
    // (`cargo make --cwd example test-browser`).
    assert_eq!(body_text(response).await, include_str!("../assets/view.js"));
}

#[tokio::test]
async fn unknown_assets_are_not_found() {
    for uri in [
        "/assets/htmx.min.js",
        "/assets/htmx-1.0.0.min.js",
        "/assets/LICENSE.htmx",
        "/assets/../src/lib.rs",
        "/assets/",
        "/assets/view.js/",
    ] {
        let response = send("GET", uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn assets_only_accept_get() {
    let response = send("POST", VIEW_SCRIPT_URL).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
