use axum::extract::{DefaultBodyLimit, Request};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::{any, get};
use axum::{Extension, Router};
use exemplum_dmn_auth::AuthComponents;
use exemplum_dmn_auth::backend::ExemplumAuthBackend;
use hypgm_authn::{jwt::JwtValidator, session::PrincipalAuthBackend};
use hypgm_http::{HealthCheck, MAX_BODY_BYTES, not_found_handler, trace_layer};
use hypgm_view::{cross_site_middleware, html_error_middleware, html_not_found_handler};
use std::sync::Arc;
use tower_sessions::SessionManagerLayer;

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

pub fn router<S>(components: AppComponents<S>) -> Router<()>
where
    S: tower_sessions::SessionStore + Clone + Send + Sync + 'static,
{
    let admin_components = exemplum_dmn_admin::AdminComponents::new(Arc::new(
        exemplum_dmn_admin::DefaultAdminService::new(
            components.account_service.clone(),
            components.authorizer,
        ),
    ));
    let admin = exemplum_dmn_admin::api::router(admin_components.clone());
    let admin_pages = exemplum_dmn_admin::view::router(admin_components);
    let hello_components = exemplum_dmn_hello_world::HelloComponents::new(
        components.hello_service,
        components.user_service,
    );
    let hello = exemplum_dmn_hello_world::controller::router(hello_components.clone());
    let hello_pages = exemplum_dmn_hello_world::view::router(hello_components);
    let auth_components = AuthComponents::new(
        components.auth_repo.clone(),
        components.account_service.clone(),
        components.token_config.clone(),
        components.password_config,
    );
    let auth_routes = exemplum_dmn_auth::controller::router(auth_components.clone());
    let auth_pages = exemplum_dmn_auth::view::router(auth_components);
    let backend = ExemplumAuthBackend::from_arc(components.auth_repo, components.account_service);

    // `/api` owns JSON errors for every path beneath it; page routes own HTML errors.
    let api = Router::new()
        .nest("/hello", hello)
        .nest("/auth", auth_routes)
        .nest("/admin", admin)
        .fallback(not_found_handler)
        .layer(axum::middleware::from_fn(api_same_origin))
        .layer(axum::middleware::from_fn_with_state(
            backend.clone(),
            exemplum_dmn_auth::backend::bearer_gate,
        ));

    let pages = Router::new()
        .merge(hello_pages)
        .merge(auth_pages)
        .route(
            "/registration-pending",
            get(exemplum_dmn_auth::view::pending),
        )
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
        .layer(axum::middleware::from_fn(cross_site_middleware))
        .layer(axum::middleware::from_fn_with_state(
            backend.clone(),
            exemplum_dmn_auth::backend::html_bearer_gate,
        ));

    let login_backend = PrincipalAuthBackend::new(backend.clone());
    let session_layer = SessionManagerLayer::new(components.session_store);
    let auth_layer = axum_login::AuthManagerLayerBuilder::new(login_backend, session_layer).build();
    let jwt_validator = JwtValidator::from_hs256_secret(&components.token_config.jwt_secret)
        .expect("application HS256 secret must contain at least 32 bytes");

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        // Nested fallbacks cover `/api` and `/api/{*path}` but not `/api/`.
        .route("/api/", any(not_found_handler))
        .merge(pages)
        .fallback(html_not_found_handler)
        .layer(auth_layer)
        .layer(Extension(jwt_validator))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(trace_layer())
}

// Apply the page origin policy to JSON writes too: login and logout can mutate
// browser sessions, and admin approvals accept those same session cookies.
// Domain routers rely on this layer rather than installing their own copy.
async fn api_same_origin(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    if !parts.method.is_safe() && hypgm_view::is_cross_site_request(&parts) {
        return hypgm_http::HttpError::from(hypgm_authz::AuthzError::Forbidden).into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}
