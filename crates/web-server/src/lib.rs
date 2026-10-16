//! Public HTTP and protobuf RPC routes, composed on one Axum listener.
use axum::{
    Router,
    http::{HeaderName, Method},
    routing::get,
};
use hello_world::{server::GrpcHelloWorldServer, service::HelloWorldService};
use std::sync::Arc;
use tower::Layer;
use tower_http::cors::{Any, CorsLayer};

/// Register public services explicitly. User CRUD and the task Board have
/// separate listeners and are intentionally absent from this router.
pub fn router(service: Arc<dyn HelloWorldService>) -> Router {
    let grpc =
        tonic_web::GrpcWebLayer::new().layer(GrpcHelloWorldServer::new(service).into_service());
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::POST, Method::OPTIONS])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("x-grpc-web"),
            HeaderName::from_static("x-user-agent"),
            HeaderName::from_static("grpc-timeout"),
        ])
        .expose_headers([
            HeaderName::from_static("grpc-status"),
            HeaderName::from_static("grpc-message"),
            HeaderName::from_static("grpc-status-details-bin"),
        ]);
    let public_rpc = tonic::service::Routes::new(grpc)
        .into_axum_router()
        .route_layer(cors);
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(public_rpc)
}
