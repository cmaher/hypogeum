use axum::extract::DefaultBodyLimit;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Router};
use exemplum_dmn_auth::AuthComponents;
use exemplum_dmn_auth::backend::ExemplumAuthBackend;
use hypgm_authn::{jwt::JwtValidator, session::PrincipalAuthBackend};
use hypgm_http::{HealthCheck, MAX_BODY_BYTES, not_found_handler, trace_layer};
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
    let hello_components = exemplum_dmn_hello_world::HelloComponents::new(
        components.hello_service,
        components.authorizer,
        components.user_service,
    );
    let hello = exemplum_dmn_hello_world::controller::router(hello_components);
    let auth_components = AuthComponents::new(
        components.auth_repo.clone(),
        components.token_config.clone(),
        components.password_config,
    );
    let auth_routes = exemplum_dmn_auth::controller::router(auth_components);

    let api = Router::new()
        .nest("/hello", hello)
        .nest("/auth", auth_routes);

    let backend = ExemplumAuthBackend::from_arc(components.auth_repo);
    let login_backend = PrincipalAuthBackend::new(backend);
    let session_layer = SessionManagerLayer::new(components.session_store);
    let auth_layer = axum_login::AuthManagerLayerBuilder::new(login_backend, session_layer).build();
    let jwt_validator = JwtValidator::from_hs256_secret(&components.token_config.jwt_secret);

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback(not_found_handler)
        .layer(auth_layer)
        .layer(Extension(jwt_validator))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(trace_layer())
}
