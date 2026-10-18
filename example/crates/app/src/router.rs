use axum::extract::DefaultBodyLimit;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Router};
use exemplum_dmn_auth::AuthState;
use exemplum_dmn_auth::backend::ExemplumAuthBackend;
use hypgm_authn::{jwt::JwtValidator, session::PrincipalAuthBackend};
use hypgm_http::{HealthCheck, MAX_BODY_BYTES, not_found_handler, trace_layer};
use tower_sessions::SessionManagerLayer;

use crate::state::AppState;

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

pub fn router<S>(state: AppState<S>) -> Router<()>
where
    S: tower_sessions::SessionStore + Clone + Send + Sync + 'static,
{
    let hello = exemplum_dmn_hello_world::controller::router(
        state.hello_service,
        state.authorizer,
        state.user_service,
    );
    let auth_state = AuthState::new(state.auth_repo.clone(), state.jwt_secret.clone())
        .with_ttl(state.token_ttl.default_ttl, state.token_ttl.max_ttl)
        .with_api_key_prefix(state.api_key_prefix);
    let auth_routes = exemplum_dmn_auth::controller::router(auth_state);

    let api = Router::new()
        .nest("/hello", hello)
        .nest("/auth", auth_routes);

    let backend = ExemplumAuthBackend::from_arc(state.auth_repo);
    let login_backend = PrincipalAuthBackend::new(backend);
    let session_layer = SessionManagerLayer::new(state.session_store);
    let auth_layer = axum_login::AuthManagerLayerBuilder::new(login_backend, session_layer).build();
    let jwt_validator = JwtValidator::from_hs256_secret(&state.jwt_secret);

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback(not_found_handler)
        .layer(auth_layer)
        .layer(Extension(jwt_validator))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(trace_layer())
}
