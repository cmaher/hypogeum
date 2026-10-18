use crate::api::{HelloRequest, HelloResponse};
use crate::service::HelloWorldService;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use exemplum_dmn_users::service::UserService;
use hypgm_authn::RequireAuth;
use hypgm_authz::Authorizer;
use hypgm_http::{AppJson, HttpError};
use std::sync::Arc;

#[derive(Clone)]
pub struct HelloState {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub authorizer: Arc<Authorizer>,
    pub user_service: Arc<dyn UserService>,
}

impl HelloState {
    pub fn new(
        hello_service: Arc<dyn HelloWorldService>,
        authorizer: Arc<Authorizer>,
        user_service: Arc<dyn UserService>,
    ) -> Self {
        Self {
            hello_service,
            authorizer,
            user_service,
        }
    }
}

pub fn router(
    service: Arc<dyn HelloWorldService>,
    authorizer: Arc<Authorizer>,
    user_service: Arc<dyn UserService>,
) -> Router<()> {
    router_with_state(HelloState::new(service, authorizer, user_service))
}

pub fn router_with_state(state: HelloState) -> Router<()> {
    Router::new()
        .route("/", post(say_hello))
        .route("/me", get(hello_me))
        .with_state(state)
}

async fn say_hello(
    State(state): State<HelloState>,
    AppJson(req): AppJson<HelloRequest>,
) -> Result<AppJson<HelloResponse>, HttpError> {
    let message = state.hello_service.say_hello(&req.name).await?;
    Ok(AppJson(HelloResponse { message }))
}

async fn hello_me(
    State(state): State<HelloState>,
    RequireAuth(principal): RequireAuth,
) -> Result<AppJson<HelloResponse>, HttpError> {
    state
        .authorizer
        .authorize(&principal, ("Action", "greet"), ("Resource", "hello_world"))
        .map_err(HttpError::from)?;

    let user = state
        .user_service
        .get(principal.user_id)
        .await
        .map_err(|e| match e {
            exemplum_dmn_users::service::UserError::NotFound => HttpError::custom(
                axum::http::StatusCode::NOT_FOUND,
                "not_found",
                "User not found",
            ),
            other => HttpError::from(other),
        })?;

    let message = format!("Hello, {}!", user.display_name);
    Ok(AppJson(HelloResponse { message }))
}
