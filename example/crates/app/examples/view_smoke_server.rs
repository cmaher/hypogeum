//! Database-free server for browser smoke checks of the HTML views.
//!
//! Uses the default in-memory component factory. Submitting the name
//! `server-error` makes the injected greeting service fail so tests can
//! observe server-error feedback. Not for production use.
//!
//! ```sh
//! cargo run --locked -p exemplum-app --example view_smoke_server -- 127.0.0.1:18080
//! ```

use async_trait::async_trait;
use exemplum_dmn_hello_world::HelloWorldError;
use exemplum_dmn_hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use std::net::SocketAddr;
use std::sync::Arc;

const SERVER_ERROR_NAME: &str = "server-error";

struct SmokeHelloWorldService;

#[async_trait]
impl HelloWorldService for SmokeHelloWorldService {
    async fn say_hello(&self, name: &str) -> Result<String, HelloWorldError> {
        if name.trim() == SERVER_ERROR_NAME {
            return Err(HelloWorldError::Internal("smoke test failure".into()));
        }
        DefaultHelloWorldService.say_hello(name).await
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind: SocketAddr = std::env::args()
        .nth(1)
        .as_deref()
        .unwrap_or("127.0.0.1:18080")
        .parse()
        .map_err(std::io::Error::other)?;
    let components = exemplum_app::AppComponentsFactory {
        hello_service: Arc::new(SmokeHelloWorldService),
        ..Default::default()
    }
    .build();
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!(
        "view smoke server listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, exemplum_app::router(components))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}
