use crate::api::{HelloRequest, HelloResponse};
use crate::service::HelloWorldService;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use exemplum_dmn_users::service::UserService;
use hypgm_authn::RequireAuth;
use hypgm_authz::{Authorizer, Decision};
use hypgm_http::{AppJson, HttpError};
use std::sync::Arc;

#[derive(Clone)]
pub struct HelloState {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub authorizer: Option<Arc<Authorizer>>,
    pub user_service: Option<Arc<dyn UserService>>,
}

pub fn router(service: Arc<dyn HelloWorldService>) -> Router<()> {
    router_with_state(HelloState {
        hello_service: service,
        authorizer: None,
        user_service: None,
    })
}

pub fn router_with_auth(
    service: Arc<dyn HelloWorldService>,
    authorizer: Arc<Authorizer>,
    user_service: Arc<dyn UserService>,
) -> Router<()> {
    router_with_state(HelloState {
        hello_service: service,
        authorizer: Some(authorizer),
        user_service: Some(user_service),
    })
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
    let authorizer = state
        .authorizer
        .as_ref()
        .ok_or_else(HttpError::internal_error)?;
    let user_service = state
        .user_service
        .as_ref()
        .ok_or_else(HttpError::internal_error)?;

    let mut decision =
        authorizer.is_authorized(&principal, ("Action", "greet"), ("Resource", "hello_world"));
    if decision != Decision::Allow {
        let alt =
            authorizer.is_authorized(&principal, ("Action", "greet"), ("Greeting", "hello_world"));
        if alt == Decision::Allow {
            decision = alt;
        }
    }

    if decision != Decision::Allow {
        return Err(hypgm_authz::AuthzError::Forbidden.into());
    }

    let user = user_service
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
