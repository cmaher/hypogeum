use crate::api::{HelloRequest, HelloResponse};
use crate::service::HelloWorldService;
use axum::extract::State;
use axum::routing::post;
use axum::Router;
use http_support::{AppJson, HttpError};
use std::sync::Arc;

pub fn router(service: Arc<dyn HelloWorldService>) -> Router<()> {
    Router::new()
        .route("/", post(say_hello))
        .with_state(service)
}

async fn say_hello(
    State(service): State<Arc<dyn HelloWorldService>>,
    AppJson(req): AppJson<HelloRequest>,
) -> Result<AppJson<HelloResponse>, HttpError> {
    let message = service.say_hello(&req.name).await?;
    Ok(AppJson(HelloResponse { message }))
}
