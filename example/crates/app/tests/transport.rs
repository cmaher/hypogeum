use exemplum_app::router;
use exemplum_dmn_hello_world::service::{DefaultHelloWorldService, HelloWorldService};
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
        let components = exemplum_app::AppComponentsFactory {
            hello_service: service,
            ..Default::default()
        }
        .build();
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
        assert!(
            !resp
                .headers()
                .contains_key("access-control-allow-credentials")
        );
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

#[tokio::test]
async fn test_malformed_shape_and_content_type_errors() {
    let server = Server::start().await;
    let client = reqwest::Client::new();

    // 1. Malformed JSON
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/json")
        .body(r#"{"name": broken"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "invalid_request");
    assert_eq!(json["message"], "Invalid request");
    assert!(!text.contains("broken"));

    // 2. Shape invalid (expected string name, got integer)
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/json")
        .body(r#"{"name": 12345}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "invalid_request");
    assert_eq!(json["message"], "Invalid request");

    // 3. Empty body
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/json")
        .body("")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "invalid_request");

    // 4. Unsupported content-type (text/plain)
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "text/plain")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 415);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "unsupported_media_type");
    assert_eq!(json["message"], "Unsupported media type");

    // 5. Malformed content-type (application/not/a/type+json)
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/not/a/type+json")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 415);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["code"], "unsupported_media_type");
    assert_eq!(json["message"], "Unsupported media type");

    server.shutdown().await;
}

#[tokio::test]
async fn test_chunked_streamed_16kib_boundary() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let server = Server::start().await;
    let addr = server.endpoint.strip_prefix("http://").unwrap();

    // 1. Exactly 16,384 bytes chunked succeeds 200 OK
    let mut chunk = b"{\"name\":\"Ada\"}".to_vec();
    chunk.resize(16_384, b' ');

    let mut tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST /api/hello HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        chunk.len()
    );
    tcp.write_all(req.as_bytes()).await.unwrap();
    tcp.write_all(&chunk).await.unwrap();
    tcp.write_all(b"\r\n0\r\n\r\n").await.unwrap();

    let mut resp = Vec::new();
    tcp.read_to_end(&mut resp).await.unwrap();
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(resp_str.starts_with("HTTP/1.1 200 OK"));

    // 2. 16,385 bytes chunked fails 413 Payload Too Large
    let mut large_chunk = b"{\"name\":\"Ada\"}".to_vec();
    large_chunk.resize(16_385, b' ');

    let mut tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST /api/hello HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        large_chunk.len()
    );
    tcp.write_all(req.as_bytes()).await.unwrap();
    tcp.write_all(&large_chunk).await.unwrap();
    tcp.write_all(b"\r\n0\r\n\r\n").await.unwrap();

    let mut resp = Vec::new();
    tcp.read_to_end(&mut resp).await.unwrap();
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(resp_str.starts_with("HTTP/1.1 413 Payload Too Large"));
    assert!(resp_str.contains(r#""code":"payload_too_large""#));

    server.shutdown().await;
}

struct PausedService {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl HelloWorldService for PausedService {
    async fn say_hello(
        &self,
        name: &str,
    ) -> Result<String, exemplum_dmn_hello_world::HelloWorldError> {
        self.started.notify_one();
        self.release.notified().await;
        DefaultHelloWorldService.say_hello(name).await
    }
}

#[tokio::test]
async fn test_graceful_shutdown_drains_in_flight_request() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());

    let server = Server::start_with(Arc::new(PausedService {
        started: started.clone(),
        release: release.clone(),
    }))
    .await;

    let client = reqwest::Client::new();
    let endpoint = server.endpoint.clone();
    let target_endpoint = endpoint.clone();

    let request_task = tokio::spawn(async move {
        client
            .post(format!("{target_endpoint}/api/hello"))
            .header("content-type", "application/json")
            .body(r#"{"name":"Ada"}"#)
            .send()
            .await
            .unwrap()
    });

    // Wait until the request has reached the service
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();

    // Signal server shutdown
    let _ = server.stop.send(());

    // Give server a moment to begin draining; verify it has not exited yet
    let mut serving = server.serving;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut serving)
            .await
            .is_err(),
        "server exited while the in-flight request was still processing"
    );

    // Release the paused request
    release.notify_one();

    // In-flight request completes successfully
    let resp = tokio::time::timeout(Duration::from_secs(5), request_task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["message"], "Hello, Ada!");

    // Server finishes shutdown and listener is released
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let addr = endpoint.strip_prefix("http://").unwrap();
    assert!(
        TcpListener::bind(addr).await.is_ok(),
        "listener must be released after graceful shutdown"
    );
}
