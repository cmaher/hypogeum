use exemplum_dmn_auth::{BrowserUrl, FlowClient, SessionVerifier};
use exemplum_dmn_hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use exemplum_dmn_users::accounts::{AccountService, DefaultAccountService};
use exemplum_dmn_users::models::{PageRequest, User, UserPage};
use exemplum_dmn_users::repo::{PgAccountRepo, PgUserRepo};
use exemplum_dmn_users::service::{DefaultUserService, UserError, UserService};
use hypgmx_kratos::KratosClient;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub struct AppComponents {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub user_service: Arc<dyn UserService>,
    pub account_service: Arc<dyn AccountService>,
    pub sessions: Arc<dyn SessionVerifier>,
    pub flows: Arc<dyn FlowClient>,
    pub kratos_browser_url: BrowserUrl,
}

impl AppComponents {
    pub fn new(
        hello_service: Arc<dyn HelloWorldService>,
        user_service: Arc<dyn UserService>,
        account_service: Arc<dyn AccountService>,
        sessions: Arc<dyn SessionVerifier>,
        flows: Arc<dyn FlowClient>,
        kratos_browser_url: BrowserUrl,
    ) -> Self {
        Self {
            hello_service,
            user_service,
            account_service,
            sessions,
            flows,
            kratos_browser_url,
        }
    }
}

/// In-memory defaults: no valid identity sessions and approved fixture accounts.
pub struct AppComponentsFactory {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub user_service: Arc<dyn UserService>,
    pub account_service: Arc<dyn AccountService>,
    pub sessions: Arc<dyn SessionVerifier>,
    pub flows: Arc<dyn FlowClient>,
    pub kratos_browser_url: BrowserUrl,
}

impl Default for AppComponentsFactory {
    fn default() -> Self {
        let auth = exemplum_dmn_auth::AuthComponentsFactory::default();
        Self {
            hello_service: Arc::new(DefaultHelloWorldService),
            user_service: Arc::new(DefaultMockUserService),
            account_service: auth.accounts,
            sessions: auth.sessions,
            flows: auth.flows,
            kratos_browser_url: auth.browser_url,
        }
    }
}

impl AppComponentsFactory {
    pub fn build(self) -> AppComponents {
        AppComponents::new(
            self.hello_service,
            self.user_service,
            self.account_service,
            self.sessions,
            self.flows,
            self.kratos_browser_url,
        )
    }

    pub fn from_pool(
        pool: PgPool,
        kratos: KratosClient,
        kratos_browser_url: BrowserUrl,
    ) -> AppComponents {
        let kratos = Arc::new(kratos);
        AppComponents::new(
            Arc::new(DefaultHelloWorldService),
            Arc::new(DefaultUserService::new(Arc::new(PgUserRepo::new(
                pool.clone(),
            )))),
            Arc::new(DefaultAccountService::new(Arc::new(PgAccountRepo::new(
                pool,
            )))),
            kratos.clone(),
            kratos,
            kratos_browser_url,
        )
    }
}

struct DefaultMockUserService;

#[async_trait::async_trait]
impl UserService for DefaultMockUserService {
    async fn create(&self, _name: &str) -> Result<User, UserError> {
        Err(UserError::NotFound)
    }

    async fn get(&self, id: Uuid) -> Result<User, UserError> {
        Ok(User {
            id,
            display_name: "Default User".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
    }

    async fn list(&self, _page: PageRequest) -> Result<UserPage, UserError> {
        Err(UserError::NotFound)
    }

    async fn update_name(&self, _id: Uuid, _name: &str) -> Result<User, UserError> {
        Err(UserError::NotFound)
    }

    async fn delete(&self, _id: Uuid) -> Result<(), UserError> {
        Ok(())
    }
}
