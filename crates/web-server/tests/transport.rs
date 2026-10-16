use base64::{Engine, engine::general_purpose::STANDARD};
use hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use prost::Message;
use rpc::public::{HelloReply, HelloRequest, public_hello_world_client::PublicHelloWorldClient};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

const RPC_PATH: &str = "/hypogeum.public.v1.PublicHelloWorld/SayHello";

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
            axum::serve(listener, web_server::router(service))
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
        self.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            TcpListener::bind(self.endpoint.strip_prefix("http://").unwrap())
                .await
                .is_ok()
        );
    }
}

fn frame(name: &str) -> Vec<u8> {
    let message = HelloRequest { name: name.into() }.encode_to_vec();
    let mut body = vec![0];
    body.extend_from_slice(&(message.len() as u32).to_be_bytes());
    body.extend(message);
    body
}

// Decode both message and trailer frames; HTTP 200 alone says nothing about RPC success.
fn decode_frames(mut body: &[u8]) -> (Vec<HelloReply>, String) {
    let mut replies = Vec::new();
    let mut trailers = String::new();
    while !body.is_empty() {
        assert!(body.len() >= 5);
        let flag = body[0];
        let length = u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize;
        assert!(body.len() >= 5 + length);
        let payload = &body[5..5 + length];
        match flag {
            0 => replies.push(HelloReply::decode(payload).unwrap()),
            0x80 => trailers.push_str(std::str::from_utf8(payload).unwrap()),
            other => panic!("unexpected frame flag: {other}"),
        }
        body = &body[5 + length..];
    }
    assert!(!trailers.is_empty(), "missing gRPC-Web status trailers");
    (replies, trailers)
}

#[tokio::test]
async fn one_listener_serves_http_native_and_binary_and_text_grpc_web() {
    let server = Server::start().await;
    let http = reqwest::Client::builder()
        .http1_only()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let health = http
        .get(format!("{}/healthz", server.endpoint))
        .send()
        .await
        .unwrap();
    assert_eq!(health.version(), reqwest::Version::HTTP_11);
    assert_eq!(health.status(), 200);
    assert_eq!(health.text().await.unwrap(), "ok");
    let mut native = PublicHelloWorldClient::connect(server.endpoint.clone())
        .await
        .unwrap();
    for (name, expected, status) in [
        ("  世界 🦀  ", Some("Hello, 世界 🦀!"), 0),
        ("", Some("Hello, world!"), 0),
        ("\0", None, 3),
    ] {
        let reply = native.say_hello(HelloRequest { name: name.into() }).await;
        if let Some(expected) = expected {
            assert_eq!(reply.unwrap().into_inner().message, expected);
        } else {
            assert_eq!(reply.unwrap_err().code(), tonic::Code::InvalidArgument);
        }
        for text in [false, true] {
            let content_type = if text {
                "application/grpc-web-text+proto"
            } else {
                "application/grpc-web+proto"
            };
            let body = if text {
                STANDARD.encode(frame(name)).into_bytes()
            } else {
                frame(name)
            };
            let response = http
                .post(format!("{}{RPC_PATH}", server.endpoint))
                .header("content-type", content_type)
                .header("accept", content_type)
                .header("x-grpc-web", "1")
                .header("origin", "https://example.test")
                .body(body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.version(), reqwest::Version::HTTP_11);
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            assert!(
                !response
                    .headers()
                    .contains_key("access-control-allow-credentials")
            );
            let exposed = response.headers()["access-control-expose-headers"]
                .to_str()
                .unwrap();
            for header in ["grpc-status", "grpc-message", "grpc-status-details-bin"] {
                assert!(exposed.contains(header));
            }
            // Tonic emits immediate failures as trailers-only response headers.
            // tonic-web preserves those headers rather than manufacturing body frames.
            let header_status = response.headers().get("grpc-status").cloned();
            let body = response.bytes().await.unwrap();
            if let Some(header_status) = header_status {
                assert_eq!(header_status.to_str().unwrap(), status.to_string());
                assert_ne!(status, 0);
                assert!(body.is_empty());
                continue;
            }
            // Each upstream data/trailer chunk can have its own base64 padding.
            let decoded = if text {
                body.chunks(4)
                    .flat_map(|chunk| STANDARD.decode(chunk).unwrap())
                    .collect()
            } else {
                body.to_vec()
            };
            let (replies, trailers) = decode_frames(&decoded);
            assert!(
                trailers
                    .lines()
                    .any(|line| line.trim() == format!("grpc-status:{status}")),
                "{trailers}"
            );
            if let Some(expected) = expected {
                assert_eq!(replies.len(), 1);
                assert_eq!(replies[0].message, expected);
            } else {
                assert!(replies.is_empty());
            }
        }
    }
    drop(native);
    drop(http);
    server.shutdown().await;
}

#[tokio::test]
async fn cors_is_limited_to_public_rpc_routes() {
    let server = Server::start().await;
    let http = reqwest::Client::new();
    for (path, public) in [
        (RPC_PATH, true),
        ("/healthz", false),
        ("/hypogeum.users.v1.Users/GetUser", false),
    ] {
        let response = http
            .request(
                reqwest::Method::OPTIONS,
                format!("{}{path}", server.endpoint),
            )
            .header("origin", "https://example.test")
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "content-type,x-grpc-web,x-user-agent,grpc-timeout",
            )
            .send()
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            public
        );
        if public {
            assert!(response.status().is_success());
            let methods = response.headers()["access-control-allow-methods"]
                .to_str()
                .unwrap();
            assert!(methods.contains("POST") && methods.contains("OPTIONS"));
            let headers = response.headers()["access-control-allow-headers"]
                .to_str()
                .unwrap();
            for name in ["content-type", "x-grpc-web", "x-user-agent", "grpc-timeout"] {
                assert!(headers.contains(name));
            }
        }
    }
    drop(http);
    server.shutdown().await;
}

#[tokio::test]
async fn unknown_services_and_methods_and_user_rpc_are_unimplemented() {
    let server = Server::start().await;
    let channel = tonic::transport::Endpoint::from_shared(server.endpoint.clone())
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut users = rpc::users::users_client::UsersClient::new(channel.clone());
    assert_eq!(
        users
            .get_user(rpc::users::GetUserRequest { id: "bad".into() })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unimplemented
    );
    let http = reqwest::Client::new();
    for path in [
        "/unknown.Service/Call",
        "/hypogeum.public.v1.PublicHelloWorld/Unknown",
        "/hypogeum.users.v1.Users/GetUser",
    ] {
        let mut grpc = tonic::client::Grpc::new(channel.clone());
        grpc.ready().await.unwrap();
        let result: Result<tonic::Response<HelloReply>, _> = grpc
            .unary(
                tonic::Request::new(HelloRequest::default()),
                path.parse().unwrap(),
                tonic_prost::ProstCodec::default(),
            )
            .await;
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unimplemented);
        let response = http
            .post(format!("{}{path}", server.endpoint))
            .header("content-type", "application/grpc-web+proto")
            .body(frame(""))
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()["grpc-status"], "12");
        assert!(response.bytes().await.unwrap().is_empty());
    }
    drop(users);
    drop(channel);
    drop(http);
    server.shutdown().await;
}

struct PausedService {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[tonic::async_trait]
impl HelloWorldService for PausedService {
    async fn say_hello(&self, name: &str) -> Result<String, errors::ServiceError> {
        self.started.notify_one();
        self.release.notified().await;
        DefaultHelloWorldService.say_hello(name).await
    }
}

#[tokio::test]
async fn graceful_shutdown_finishes_an_active_rpc() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let server = Server::start_with(Arc::new(PausedService {
        started: started.clone(),
        release: release.clone(),
    }))
    .await;
    let mut client = PublicHelloWorldClient::connect(server.endpoint.clone())
        .await
        .unwrap();
    let request =
        tokio::spawn(async move { client.say_hello(HelloRequest { name: "Ada".into() }).await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    server.stop.send(()).unwrap();
    let mut serving = server.serving;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut serving)
            .await
            .is_err(),
        "server exited while the RPC was still active"
    );
    release.notify_one();
    let reply = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reply.into_inner().message, "Hello, Ada!");
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
