use exemplum_app::router;
use exemplum_dmn_hello_world::service::NAME_VALIDATION_MESSAGE;
use hypgm_view::{CROSS_SITE_FORM, HTML_CONTENT_TYPE, HTMX_SCRIPT_URL, VIEW_SCRIPT_URL};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const FORM: &str = "application/x-www-form-urlencoded";

struct Server {
    endpoint: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let components = exemplum_app::AppComponentsFactory::default().build();
        let serving = tokio::spawn(async move {
            axum::serve(listener, router(components))
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            endpoint,
            stop,
            serving,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint)
    }

    async fn post_form(&self, body: &str, htmx: bool) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .post(self.url("/hello"))
            .header("content-type", FORM)
            .body(body.to_owned());
        if htmx {
            request = request.header("hx-request", "true");
        }
        request.send().await.unwrap()
    }

    async fn shutdown(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(5), self.serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> &'a str {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .to_str()
        .unwrap()
}

fn assert_html(response: &reqwest::Response, status: u16) {
    assert_eq!(response.status(), status);
    assert_eq!(header(response, "content-type"), HTML_CONTENT_TYPE);
    assert!(!header(response, "x-request-id").is_empty());
}

fn assert_htmx_vary(response: &reqwest::Response) {
    let vary: Vec<_> = response
        .headers()
        .get_all("vary")
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert_eq!(vary, ["HX-Request", "HX-History-Restore-Request"]);
}

async fn assert_html_error(response: reqwest::Response, status: u16, message: &str) {
    assert_html(&response, status);
    let body = response.text().await.unwrap();
    assert!(body.contains("data-view-error"), "{body}");
    assert!(body.contains(message), "{body}");
}

async fn assert_json_error(response: reqwest::Response, status: u16, code: &str) {
    assert_eq!(response.status(), status);
    assert!(!header(&response, "x-request-id").is_empty());
    let json: serde_json::Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(json["code"], code);
}

#[tokio::test]
async fn root_serves_full_page_with_shared_assets() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    let response = client.get(server.url("/")).send().await.unwrap();
    assert_html(&response, 200);
    assert_htmx_vary(&response);
    assert_eq!(header(&response, "x-frame-options"), "DENY");
    assert_eq!(header(&response, "x-content-type-options"), "nosniff");
    let body = response.text().await.unwrap();
    assert!(body.starts_with("<!DOCTYPE html>"));
    assert!(body.contains(r#"method="post" action="/hello""#));
    assert!(body.contains(HTMX_SCRIPT_URL));
    assert!(body.contains(VIEW_SCRIPT_URL));

    for asset in [HTMX_SCRIPT_URL, VIEW_SCRIPT_URL] {
        let response = client.get(server.url(asset)).send().await.unwrap();
        assert_eq!(response.status(), 200, "{asset}");
        assert_eq!(
            header(&response, "content-type"),
            "text/javascript; charset=utf-8"
        );
    }

    server.shutdown().await;
}

#[tokio::test]
async fn greeting_form_supports_ordinary_and_htmx_submissions() {
    let server = Server::start().await;

    let response = server.post_form("name=Ada", false).await;
    assert_html(&response, 200);
    let page = response.text().await.unwrap();
    assert!(page.starts_with("<!DOCTYPE html>"));
    assert!(page.contains("Hello, Ada!"));

    let response = server.post_form("name=Ada", true).await;
    assert_html(&response, 200);
    assert_htmx_vary(&response);
    let fragment = response.text().await.unwrap();
    assert!(fragment.starts_with("<section"));
    assert!(fragment.contains("Hello, Ada!"));
    assert!(page.contains(fragment.trim()));

    let response = server.post_form("name=", true).await;
    assert!(response.text().await.unwrap().contains("Hello, world!"));

    let response = server
        .post_form("name=%3Cimg+src%3Dx+onerror%3Dalert(1)%3E", true)
        .await;
    assert_html(&response, 200);
    let body = response.text().await.unwrap();
    assert!(!body.contains("<img"));

    server.shutdown().await;
}

#[tokio::test]
async fn history_restoration_receives_full_page() {
    let server = Server::start().await;

    let response = reqwest::Client::new()
        .post(server.url("/hello"))
        .header("content-type", FORM)
        .header("hx-request", "true")
        .header("hx-history-restore-request", "true")
        .body("name=Ada")
        .send()
        .await
        .unwrap();
    assert_html(&response, 200);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .starts_with("<!DOCTYPE html>")
    );

    server.shutdown().await;
}

#[tokio::test]
async fn greeting_validation_keeps_status_and_entered_value() {
    let server = Server::start().await;
    let name = "x".repeat(201);

    for htmx in [false, true] {
        let response = server.post_form(&format!("name={name}"), htmx).await;
        assert_html(&response, 400);
        let body = response.text().await.unwrap();
        assert!(body.contains(NAME_VALIDATION_MESSAGE));
        assert!(body.contains(&format!(r#"value="{name}""#)));
        assert_eq!(body.contains("<!DOCTYPE html>"), !htmx);
    }

    server.shutdown().await;
}

#[tokio::test]
async fn form_transport_errors_are_safe_html() {
    let server = Server::start().await;

    let response = reqwest::Client::new()
        .post(server.url("/hello"))
        .header("content-type", "application/json")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .unwrap();
    assert_html_error(response, 415, "Unsupported media type").await;

    let oversized = format!("name={}", "x".repeat(16_384));
    let response = server.post_form(&oversized, true).await;
    assert_html_error(response, 413, "Payload too large").await;

    server.shutdown().await;
}

#[tokio::test]
async fn cross_site_form_posts_are_rejected() {
    let server = Server::start().await;

    let response = reqwest::Client::new()
        .post(server.url("/hello"))
        .header("content-type", FORM)
        .header("sec-fetch-site", "cross-site")
        .body("name=Ada")
        .send()
        .await
        .unwrap();
    assert_html_error(response, 403, CROSS_SITE_FORM.message).await;

    // The page-router middleware rejects before any extractor or method
    // routing runs, so bodies without a form media type are covered too.
    for (path, content_type) in [("/hello", "text/plain"), ("/", FORM)] {
        let response = reqwest::Client::new()
            .post(server.url(path))
            .header("content-type", content_type)
            .header("sec-fetch-site", "cross-site")
            .body("name=Ada")
            .send()
            .await
            .unwrap();
        assert_html_error(response, 403, CROSS_SITE_FORM.message).await;
    }

    // Safe methods and the JSON API are not affected.
    let response = reqwest::Client::new()
        .get(server.url("/"))
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_html(&response, 200);

    let response = reqwest::Client::new()
        .post(server.url("/hello"))
        .header("content-type", FORM)
        .header("origin", server.endpoint.as_str())
        .body("name=Ada")
        .send()
        .await
        .unwrap();
    assert_html(&response, 200);

    server.shutdown().await;
}

#[tokio::test]
async fn page_routes_own_html_404_and_405() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    for path in [
        "/hello/",
        "/unknown",
        "/hypogeum.public.v1.PublicHelloWorld/SayHello",
        "/assets",
        "/assets/missing.js",
        "/assets/view.js/",
    ] {
        let response = client.get(server.url(path)).send().await.unwrap();
        assert_html_error(response, 404, "Resource not found").await;
    }

    let response = client.get(server.url("/hello")).send().await.unwrap();
    assert!(header(&response, "allow").contains("POST"));
    assert_html_error(response, 405, "Method not allowed").await;

    let response = client.delete(server.url("/")).send().await.unwrap();
    let allow = header(&response, "allow").to_owned();
    assert!(allow.contains("GET"), "{allow}");
    assert!(!allow.contains("DELETE"), "{allow}");
    assert_html_error(response, 405, "Method not allowed").await;

    let response = client
        .post(server.url(VIEW_SCRIPT_URL))
        .send()
        .await
        .unwrap();
    assert!(header(&response, "allow").contains("GET"));
    assert_html_error(response, 405, "Method not allowed").await;

    server.shutdown().await;
}

#[tokio::test]
async fn api_and_health_keep_json_semantics() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    let response = client
        .post(server.url("/api/hello"))
        .header("content-type", "application/json")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(header(&response, "content-type"), "application/json");
    let json: serde_json::Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(json["message"], "Hello, Ada!");

    for path in ["/api", "/api/", "/api/missing", "/api/hello/", "/api/auth"] {
        let response = client.get(server.url(path)).send().await.unwrap();
        assert_json_error(response, 404, "not_found").await;
    }

    let response = client.get(server.url("/api/hello")).send().await.unwrap();
    assert!(header(&response, "allow").contains("POST"));
    assert_json_error(response, 405, "method_not_allowed").await;

    let response = client
        .post(server.url("/api/hello"))
        .header("content-type", FORM)
        .body("name=Ada")
        .send()
        .await
        .unwrap();
    assert_json_error(response, 415, "unsupported_media_type").await;

    let response = client.post(server.url("/healthz")).send().await.unwrap();
    assert_json_error(response, 405, "method_not_allowed").await;

    let response = client.get(server.url("/healthz")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "ok");

    server.shutdown().await;
}
