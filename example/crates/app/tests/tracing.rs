use exemplum_app::router;
use exemplum_dmn_hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use hypgm_logging::{LogFormat, create_test_dispatch};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::instrument::WithSubscriber;

struct TracingServer {
    endpoint: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<std::io::Result<()>>,
}

impl TracingServer {
    async fn start(service: Arc<dyn HelloWorldService>, dispatch: tracing::Dispatch) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let router_dispatch = dispatch.clone();
        let app = router(service).layer(axum::middleware::from_fn(
            move |req, next: axum::middleware::Next| {
                let dispatch = router_dispatch.clone();
                async move { next.run(req).with_subscriber(dispatch).await }
            },
        ));
        let serving = tokio::spawn(
            async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
            }
            .with_subscriber(dispatch),
        );
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
async fn test_concurrent_requests_tracing_and_request_ids() {
    let (dispatch, writer) = create_test_dispatch("debug", LogFormat::Json).unwrap();
    let server = TracingServer::start(Arc::new(DefaultHelloWorldService), dispatch.clone()).await;

    let client = reqwest::Client::new();
    let mut tasks = Vec::new();

    let request_count = 10;
    for i in 0..request_count {
        let client = client.clone();
        let endpoint = server.endpoint.clone();
        let untrusted_id = format!("caller-provided-id-{i}");
        let body = format!(r#"{{"name":"User{i}"}}"#);

        tasks.push(tokio::spawn(async move {
            let resp = client
                .post(format!("{endpoint}/api/hello"))
                .header("content-type", "application/json")
                .header("x-request-id", untrusted_id.clone())
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(resp.status(), 200);
            let returned_id = resp
                .headers()
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();

            assert_ne!(returned_id, untrusted_id);
            assert!(uuid::Uuid::parse_str(&returned_id).is_ok());

            (untrusted_id, returned_id)
        }));
    }

    let mut returned_ids = HashSet::new();
    let mut caller_ids = HashSet::new();

    for task in tasks {
        let (caller, returned) = task.await.unwrap();
        caller_ids.insert(caller);
        returned_ids.insert(returned);
    }

    // All returned IDs must be unique
    assert_eq!(returned_ids.len(), request_count);

    server.shutdown().await;

    // Inspect captured logs
    let lines = writer.lines();
    let mut completion_events = Vec::new();

    for line in &lines {
        let val: serde_json::Value = serde_json::from_str(line).unwrap();

        // No caller IDs or user names leaked into logged fields
        let line_str = val.to_string();
        for caller in &caller_ids {
            assert!(!line_str.contains(caller));
        }

        if val["fields"].get("message")
            == Some(&serde_json::Value::String("request completed".into()))
        {
            completion_events.push(val);
        }
    }

    assert_eq!(
        completion_events.len(),
        request_count,
        "each request must produce exactly one completion event"
    );

    for event in completion_events {
        assert_eq!(event["level"], "INFO");
        let fields = &event["fields"];
        assert_eq!(fields["status"], 200);
        assert_eq!(fields["matched_path"], "/api/hello");
        assert_eq!(fields["method"], "POST");
        assert!(fields["elapsed_ms"].is_number());
        let req_id = fields["request_id"].as_str().unwrap();
        assert!(returned_ids.contains(req_id));
    }
}

#[tokio::test]
async fn test_early_rejections_and_fallbacks_tracing() {
    let (dispatch, writer) = create_test_dispatch("debug", LogFormat::Json).unwrap();
    let server = TracingServer::start(Arc::new(DefaultHelloWorldService), dispatch.clone()).await;

    let client = reqwest::Client::new();

    // 1. Healthz (DEBUG)
    let resp = client
        .get(format!("{}/healthz", server.endpoint))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let health_req_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 2. 404 Unknown route (INFO, unmatched marker)
    let resp = client
        .get(format!(
            "{}/unknown/secret_path?query=secret",
            server.endpoint
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let not_found_req_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 3. 405 Method Not Allowed
    let resp = client
        .get(format!("{}/api/hello", server.endpoint))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
    let method_req_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 4. 400 Bad Request
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "application/json")
        .body(r#"{"name": bad"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let bad_req_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 5. 415 Unsupported Media Type
    let resp = client
        .post(format!("{}/api/hello", server.endpoint))
        .header("content-type", "text/plain")
        .body(r#"{"name":"Ada"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 415);
    let media_req_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    server.shutdown().await;

    let lines = writer.lines();
    let mut completions = std::collections::HashMap::new();

    for line in lines {
        let val: serde_json::Value = serde_json::from_str(&line).unwrap();
        if val["fields"].get("message")
            == Some(&serde_json::Value::String("request completed".into()))
        {
            let req_id = val["fields"]["request_id"].as_str().unwrap().to_string();
            completions.insert(req_id, val);
        }
    }

    // Health check verification
    let health_event = &completions[&health_req_id];
    assert_eq!(health_event["level"], "DEBUG");
    assert_eq!(health_event["fields"]["status"], 200);
    assert_eq!(health_event["fields"]["matched_path"], "/healthz");

    // 404 verification: template must be "unmatched", NO query or raw path
    let not_found_event = &completions[&not_found_req_id];
    assert_eq!(not_found_event["level"], "INFO");
    assert_eq!(not_found_event["fields"]["status"], 404);
    assert_eq!(not_found_event["fields"]["matched_path"], "unmatched");
    assert_eq!(not_found_event["fields"]["error_code"], "not_found");
    assert!(!not_found_event.to_string().contains("secret_path"));
    assert!(!not_found_event.to_string().contains("secret"));

    // 405 verification
    let method_event = &completions[&method_req_id];
    assert_eq!(method_event["level"], "INFO");
    assert_eq!(method_event["fields"]["status"], 405);
    assert_eq!(method_event["fields"]["matched_path"], "/api/hello");
    assert_eq!(method_event["fields"]["error_code"], "method_not_allowed");

    // 400 verification
    let bad_event = &completions[&bad_req_id];
    assert_eq!(bad_event["level"], "INFO");
    assert_eq!(bad_event["fields"]["status"], 400);
    assert_eq!(bad_event["fields"]["error_code"], "invalid_request");

    // 415 verification
    let media_event = &completions[&media_req_id];
    assert_eq!(media_event["level"], "INFO");
    assert_eq!(media_event["fields"]["status"], 415);
    assert_eq!(
        media_event["fields"]["error_code"],
        "unsupported_media_type"
    );
}
