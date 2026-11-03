use axum::extract::{DefaultBodyLimit, Request};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::{any, get};
use axum::{Extension, Router};
use exemplum_dmn_auth::AuthComponents;
use hypgm_http::{HealthCheck, MAX_BODY_BYTES, not_found_handler, trace_layer};
use hypgm_view::{cross_site_middleware, html_error_middleware, html_not_found_handler};
use std::sync::Arc;

use crate::components::AppComponents;

async fn healthz() -> impl IntoResponse {
    (
        Extension(HealthCheck),
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        "ok",
    )
}

pub fn router(components: AppComponents) -> Router<()> {
    let admin_components = exemplum_dmn_admin::AdminComponents::new(Arc::new(
        exemplum_dmn_admin::DefaultAdminService::new(components.account_service.clone()),
    ));
    let admin = exemplum_dmn_admin::api::router(admin_components.clone());
    let admin_pages = exemplum_dmn_admin::view::router(admin_components);
    let hello_components = exemplum_dmn_hello_world::HelloComponents::new(
        components.hello_service,
        components.user_service,
    );
    let hello = exemplum_dmn_hello_world::controller::router(hello_components.clone());
    let hello_pages = exemplum_dmn_hello_world::view::router(hello_components);
    let auth = AuthComponents::new(
        components.sessions,
        components.flows,
        components.account_service,
        components.kratos_browser_url,
    );
    let auth_pages = exemplum_dmn_auth::router(auth.clone());

    // `/api` owns JSON errors for every path beneath it; page routes own HTML errors.
    let api = Router::new()
        .nest("/hello", hello)
        .nest("/admin", admin)
        .fallback(not_found_handler)
        .layer(axum::middleware::from_fn(api_same_origin));

    let pages = Router::new()
        .merge(hello_pages)
        .merge(auth_pages)
        .route(
            "/admin",
            get(|| async { axum::response::Redirect::to("/admin/users") }),
        )
        .nest("/admin", admin_pages)
        .nest(
            hypgm_view::assets::ASSETS_MOUNT,
            hypgm_view::assets::router(),
        )
        .layer(axum::middleware::from_fn(html_error_middleware))
        .layer(axum::middleware::from_fn(cross_site_middleware));

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        // Nested fallbacks cover `/api` and `/api/{*path}` but not `/api/`.
        .route("/api/", any(not_found_handler))
        .merge(pages)
        .fallback(html_not_found_handler)
        // Resolves the identity session for every route; see `session_gate`.
        .layer(axum::middleware::from_fn_with_state(
            auth,
            exemplum_dmn_auth::session_gate,
        ))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(trace_layer())
}

// Apply the page origin policy to JSON writes too: admin approvals accept the
// same browser session cookie as pages.
// Domain routers rely on this layer rather than installing their own copy.
async fn api_same_origin(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    if !parts.method.is_safe() && hypgm_view::is_cross_site_request(&parts) {
        return hypgm_http::HttpError::from(hypgm_errors::PublicErrorInfo::FORBIDDEN)
            .into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}
