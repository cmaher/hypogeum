use crate::api::{HelloRequest, HelloResponse};
use crate::service::HelloWorldService;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use exemplum_dmn_users::service::UserService;
use hypgm_authn::RequireAuth;
use hypgm_http::{AppJson, HttpError};
use std::sync::Arc;

#[derive(Clone)]
pub struct HelloComponents {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub user_service: Arc<dyn UserService>,
}

impl HelloComponents {
    pub fn new(
        hello_service: Arc<dyn HelloWorldService>,
        user_service: Arc<dyn UserService>,
    ) -> Self {
        Self {
            hello_service,
            user_service,
        }
    }
}

pub struct HelloComponentsFactory {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub user_service: Arc<dyn UserService>,
}

impl Default for HelloComponentsFactory {
    fn default() -> Self {
        Self {
            hello_service: Arc::new(crate::service::DefaultHelloWorldService),
            user_service: Arc::new(DefaultMockUserService),
        }
    }
}

impl HelloComponentsFactory {
    pub fn build(self) -> HelloComponents {
        HelloComponents::new(self.hello_service, self.user_service)
    }
}

struct DefaultMockUserService;

#[async_trait::async_trait]
impl UserService for DefaultMockUserService {
    async fn create(
        &self,
        _name: &str,
    ) -> Result<exemplum_dmn_users::models::User, exemplum_dmn_users::service::UserError> {
        Err(exemplum_dmn_users::service::UserError::NotFound)
    }

    async fn get(
        &self,
        id: uuid::Uuid,
    ) -> Result<exemplum_dmn_users::models::User, exemplum_dmn_users::service::UserError> {
        Ok(exemplum_dmn_users::models::User {
            id,
            display_name: "Default User".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
    }

    async fn list(
        &self,
        _page: exemplum_dmn_users::models::PageRequest,
    ) -> Result<exemplum_dmn_users::models::UserPage, exemplum_dmn_users::service::UserError> {
        Err(exemplum_dmn_users::service::UserError::NotFound)
    }

    async fn update_name(
        &self,
        _id: uuid::Uuid,
        _name: &str,
    ) -> Result<exemplum_dmn_users::models::User, exemplum_dmn_users::service::UserError> {
        Err(exemplum_dmn_users::service::UserError::NotFound)
    }

    async fn delete(&self, _id: uuid::Uuid) -> Result<(), exemplum_dmn_users::service::UserError> {
        Ok(())
    }
}

pub fn router(components: HelloComponents) -> Router<()> {
    Router::new()
        .route("/", post(say_hello))
        .route("/me", get(hello_me))
        .with_state(components)
}

async fn say_hello(
    State(components): State<HelloComponents>,
    AppJson(req): AppJson<HelloRequest>,
) -> Result<AppJson<HelloResponse>, HttpError> {
    let message = components.hello_service.say_hello(&req.name).await?;
    Ok(AppJson(HelloResponse { message }))
}

async fn hello_me(
    State(components): State<HelloComponents>,
    RequireAuth(principal): RequireAuth,
) -> Result<AppJson<HelloResponse>, HttpError> {
    let user = components
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
