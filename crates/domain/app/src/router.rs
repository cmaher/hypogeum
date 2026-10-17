use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use hello_world::service::HelloWorldService;
use http_support::{MAX_BODY_BYTES, not_found_handler, trace_layer};
use std::sync::Arc;

async fn healthz() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "ok")
}

pub fn router(hello_service: Arc<dyn HelloWorldService>) -> Router<()> {
    let hello = hello_world::controller::router(hello_service);
    let api = Router::new().nest("/hello", hello);

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback(not_found_handler)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(trace_layer())
}
