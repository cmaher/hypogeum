use exemplum_dmn_auth::PostgresSessionStore;
pub use exemplum_dmn_auth::TokenConfig;
use exemplum_dmn_auth::repo::{AuthRepo, InMemoryAuthRepo, PgAuthRepo};
use exemplum_dmn_hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use exemplum_dmn_users::models::{PageRequest, User, UserPage};
use exemplum_dmn_users::repo::PgUserRepo;
use exemplum_dmn_users::service::{DefaultUserService, UserError, UserService};
use hypgm_authz::Authorizer;
use sqlx::PgPool;
use std::sync::Arc;
use tower_sessions::SessionStore;
use uuid::Uuid;

pub struct AppComponents<S> {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub user_service: Arc<dyn UserService>,
    pub auth_repo: Arc<dyn AuthRepo>,
    pub authorizer: Arc<Authorizer>,
    pub session_store: S,
    pub token_config: TokenConfig,
}

impl<S: SessionStore + Clone + Send + Sync + 'static> AppComponents<S> {
    pub fn new(
        hello_service: Arc<dyn HelloWorldService>,
        user_service: Arc<dyn UserService>,
        auth_repo: Arc<dyn AuthRepo>,
        authorizer: Arc<Authorizer>,
        session_store: S,
        token_config: TokenConfig,
    ) -> Self {
        Self {
            hello_service,
            user_service,
            auth_repo,
            authorizer,
            session_store,
            token_config,
        }
    }
}

pub struct AppComponentsFactory<S = tower_sessions::MemoryStore> {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub user_service: Arc<dyn UserService>,
    pub auth_repo: Arc<dyn AuthRepo>,
    pub authorizer: Arc<Authorizer>,
    pub session_store: S,
    pub token_config: TokenConfig,
}

impl Default for AppComponentsFactory<tower_sessions::MemoryStore> {
    fn default() -> Self {
        Self {
            hello_service: Arc::new(DefaultHelloWorldService),
            user_service: Arc::new(DefaultMockUserService),
            auth_repo: Arc::new(InMemoryAuthRepo::new()),
            authorizer: default_authorizer(),
            session_store: tower_sessions::MemoryStore::default(),
            token_config: TokenConfig::default(),
        }
    }
}

impl<S: SessionStore + Clone + Send + Sync + 'static> AppComponentsFactory<S> {
    pub fn build(self) -> AppComponents<S> {
        AppComponents::new(
            self.hello_service,
            self.user_service,
            self.auth_repo,
            self.authorizer,
            self.session_store,
            self.token_config,
        )
    }
}

impl AppComponentsFactory<PostgresSessionStore> {
    pub fn from_pool(
        pool: PgPool,
        authorizer: Arc<Authorizer>,
        token_config: TokenConfig,
    ) -> AppComponents<PostgresSessionStore> {
        let session_store = PostgresSessionStore::new_with_schema(pool.clone(), "exemplum");
        let auth_repo = Arc::new(PgAuthRepo::new(pool.clone()));
        let user_repo = Arc::new(PgUserRepo::new(pool));
        let user_service = Arc::new(DefaultUserService::new(user_repo));
        AppComponents::new(
            Arc::new(DefaultHelloWorldService),
            user_service,
            auth_repo,
            authorizer,
            session_store,
            token_config,
        )
    }
}

pub fn default_authorizer() -> Arc<Authorizer> {
    const DEFAULT_SCHEMA: &str = include_str!("../../../config/schema.cedarschema");
    const DEFAULT_POLICIES: &str = include_str!("../../../config/policies.cedar");
    let (schema, policies) =
        hypgm_authz::load_schema_and_policies(DEFAULT_SCHEMA, DEFAULT_POLICIES)
            .expect("embedded Cedar schema and policies are valid");
    Arc::new(Authorizer::new(schema, policies))
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
