use axum::extract::DefaultBodyLimit;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Router};
use exemplum_dmn_hello_world::service::HelloWorldService;
use exemplum_dmn_users::service::UserService;
use hypgm_authn::{jwt::JwtValidator, session::PrincipalAuthBackend};
use hypgm_authz::Authorizer;
use hypgm_http::{HealthCheck, MAX_BODY_BYTES, not_found_handler, trace_layer};
use std::sync::Arc;
use tower_sessions::SessionManagerLayer;

use crate::auth::{
    AuthState, ExemplumAuthBackend,
    repo::{AuthRepo, InMemoryAuthRepo, PgAuthRepo},
};

async fn healthz() -> impl IntoResponse {
    (
        Extension(HealthCheck),
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "ok",
    )
}

pub fn router(hello_service: Arc<dyn HelloWorldService>) -> Router<()> {
    let hello = exemplum_dmn_hello_world::controller::router(hello_service);
    let auth_repo = Arc::new(InMemoryAuthRepo::default());
    let session_store = tower_sessions::MemoryStore::default();
    let jwt_secret = b"exemplum-default-secret-key-32bytes!".to_vec();
    router_with_components(hello, auth_repo, session_store, jwt_secret)
}

pub fn router_with_auth<S>(
    hello_service: Arc<dyn HelloWorldService>,
    authorizer: Arc<Authorizer>,
    user_service: Arc<dyn UserService>,
    auth_repo: Arc<dyn AuthRepo>,
    session_store: S,
    jwt_secret: Vec<u8>,
) -> Router<()>
where
    S: tower_sessions::SessionStore + Clone + Send + Sync + 'static,
{
    let hello = exemplum_dmn_hello_world::controller::router_with_auth(
        hello_service,
        authorizer,
        user_service,
    );
    router_with_components(hello, auth_repo, session_store, jwt_secret)
}

pub fn router_with_postgres(
    hello_service: Arc<dyn HelloWorldService>,
    authorizer: Arc<Authorizer>,
    pool: sqlx::PgPool,
    jwt_secret: Vec<u8>,
) -> Router<()> {
    let session_store = crate::auth::session::init_session_store(pool.clone());
    let auth_repo = Arc::new(PgAuthRepo::new(pool.clone()));
    let user_repo = Arc::new(exemplum_dmn_users::repo::PgUserRepo::new(pool));
    let user_service = Arc::new(exemplum_dmn_users::service::DefaultUserService::new(user_repo));
    router_with_auth(
        hello_service,
        authorizer,
        user_service,
        auth_repo,
        session_store,
        jwt_secret,
    )
}

pub fn router_with_components<S, R>(
    hello_router: Router<()>,
    auth_repo: Arc<R>,
    session_store: S,
    jwt_secret: Vec<u8>,
) -> Router<()>
where
    S: tower_sessions::SessionStore + Clone + Send + Sync + 'static,
    R: AuthRepo + 'static,
{
    let auth_state = AuthState::new(auth_repo.clone(), jwt_secret.clone());
    let auth_routes = crate::auth::controller::router(auth_state);

    let api = Router::new()
        .nest("/hello", hello_router)
        .nest("/auth", auth_routes);

    let backend = ExemplumAuthBackend::from_arc(auth_repo);
    let login_backend = PrincipalAuthBackend::new(backend);
    let session_layer = SessionManagerLayer::new(session_store);
    let auth_layer = axum_login::AuthManagerLayerBuilder::new(login_backend, session_layer).build();
    let jwt_validator = JwtValidator::from_hs256_secret(&jwt_secret);

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback(not_found_handler)
        .layer(auth_layer)
        .layer(Extension(jwt_validator))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(trace_layer())
}
