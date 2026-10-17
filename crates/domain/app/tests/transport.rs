use app::router;
use hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

struct Server {
    endpoint: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(Arc::new(DefaultHelloWorldService)).await
    }

    async fn start_with(service: Arc<dyn HelloWorldService>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let serving = tokio::spawn(async move {
            axum::serve(listener, router(service))
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

    async fn shutdown(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(5), self.serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn test_healthz_get_and_head() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    // GET /healthz
    let resp = client
        .get(format!("{}/healthz", server.endpoint))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/plain; charset=utf-8"
    );
    assert_eq!(resp.text().await.unwrap(), "ok");

    // HEAD /healthz
    let resp = client
        .head(format!("{}/healthz", server.endpoint))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/plain; charset=utf-8"
    );
    assert_eq!(resp.bytes().await.unwrap().len(), 0);

    server.shutdown().await;
}

#[tokio::test]
async fn test_api_hello_canonical_and_trailing_slash_404() {
    let server = Server::start().await;
    // Don't follow redirects to verify no trailing slash redirect
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // POST /api/hello succeeds
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/json")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let request_id = resp
        .headers()
        .get("x-request-id")
        .expect("x-request-id present")
        .to_str()
        .unwrap();
    assert!(uuid::Uuid::parse_str(request_id).is_ok());

    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["message"], "Hello, Ada!");

    // POST /api/hello/ returns 404 with no redirect
    let resp = client
        .post(format!("{}/api/hello/", server.endpoint))
        .header("content-type", "application/json")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "not_found");
    assert_eq!(json["message"], "Resource not found");

    server.shutdown().await;
}

#[tokio::test]
async fn test_json_404_fallbacks() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    for path in [
        "/api",
        "/api/",
        "/unknown",
        "/hypogeum.public.v1.PublicHelloWorld/SayHello",
        "/hypogeum.users.v1.Users/GetUser",
    ] {
        let resp = client
            .get(format!("{}{path}", server.endpoint))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 404);
        assert!(resp.headers().get("x-request-id").is_some());
        let text = resp.text().await.unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["code"], "not_found");
        assert_eq!(json["message"], "Resource not found");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_json_405_and_allow_header() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    // GET /api/hello -> 405 with Allow: POST
    let resp = client
        .get(format!("{}/api/hello", server.endpoint))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 405);
    let allow = resp.headers().get("allow").unwrap().to_str().unwrap();
    assert!(allow.contains("POST"));
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "method_not_allowed");
    assert_eq!(json["message"], "Method not allowed");

    // POST /healthz -> 405 with Allow: GET, HEAD
    let resp = client
        .post(format!("{}/healthz", server.endpoint))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 405);
    let allow = resp.headers().get("allow").unwrap().to_str().unwrap();
    assert!(allow.contains("GET"));
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "method_not_allowed");
    assert_eq!(json["message"], "Method not allowed");

    server.shutdown().await;
}

#[tokio::test]
async fn test_no_implicit_options_success_and_no_cors_headers() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    for path in ["/api/hello", "/healthz", "/unknown"] {
        let resp = client
            .request(
                reqwest::Method::OPTIONS,
                format!("{}{path}", server.endpoint),
            )
            .header("origin", "https://example.com")
            .send()
            .await
            .unwrap();

        // No permissive CORS headers
        assert!(!resp.headers().contains_key("access-control-allow-origin"));
        assert!(!resp
            .headers()
            .contains_key("access-control-allow-credentials"));
        // OPTIONS is not a registered method, so it's not success
        assert!(!resp.status().is_success());
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_16kib_body_limit_policy() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    // 16,384 bytes valid padded JSON succeeds
    let prefix = b"{\"name\":\"Ada\"}";
    let mut valid_body = prefix.to_vec();
    valid_body.resize(16_384, b' ');

    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/json")
        .body(valid_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    // 16,385 bytes fails 413
    let mut large_body = prefix.to_vec();
    large_body.resize(16_385, b' ');

    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/json")
        .body(large_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 413);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "payload_too_large");
    assert_eq!(json["message"], "Payload too large");

    server.shutdown().await;
}
