use exemplum_dmn_auth::repo::{AuthRepo, InMemoryAuthRepo, PgAuthRepo};
use exemplum_dmn_hello_world::service::{DefaultHelloWorldService, HelloWorldService};
use exemplum_dmn_users::models::{PageRequest, User, UserPage};
use exemplum_dmn_users::repo::PgUserRepo;
use exemplum_dmn_users::service::{DefaultUserService, UserError, UserService};
use hypgm_authz::Authorizer;
use hypgmx_rdbms::PostgresSessionStore;
use sqlx::PgPool;
use std::sync::Arc;
use tower_sessions::SessionStore;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct TokenTtlConfig {
    pub default_ttl: usize,
    pub max_ttl: usize,
}

impl Default for TokenTtlConfig {
    fn default() -> Self {
        Self {
            default_ttl: 3600,
            max_ttl: 86400,
        }
    }
}

pub struct AppState<S> {
    pub hello_service: Arc<dyn HelloWorldService>,
    pub user_service: Arc<dyn UserService>,
    pub auth_repo: Arc<dyn AuthRepo>,
    pub authorizer: Arc<Authorizer>,
    pub session_store: S,
    pub jwt_secret: Vec<u8>,
    pub token_ttl: TokenTtlConfig,
    pub api_key_prefix: String,
}

impl<S: SessionStore + Clone + Send + Sync + 'static> AppState<S> {
    pub fn new(
        hello_service: Arc<dyn HelloWorldService>,
        user_service: Arc<dyn UserService>,
        auth_repo: Arc<dyn AuthRepo>,
        authorizer: Arc<Authorizer>,
        session_store: S,
        jwt_secret: Vec<u8>,
    ) -> Self {
        Self {
            hello_service,
            user_service,
            auth_repo,
            authorizer,
            session_store,
            jwt_secret,
            token_ttl: TokenTtlConfig::default(),
            api_key_prefix: "exmpl_key".to_string(),
        }
    }

    pub fn with_hello_service(mut self, hello_service: Arc<dyn HelloWorldService>) -> Self {
        self.hello_service = hello_service;
        self
    }

    pub fn with_token_ttl(mut self, default_ttl: usize, max_ttl: usize) -> Self {
        self.token_ttl = TokenTtlConfig {
            default_ttl,
            max_ttl,
        };
        self
    }

    pub fn with_api_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.api_key_prefix = prefix.into();
        self
    }
}

pub fn default_authorizer() -> Arc<Authorizer> {
    const DEFAULT_SCHEMA: &str = include_str!("../../../config/schema.cedarschema");
    const DEFAULT_POLICIES: &str = include_str!("../../../config/policies.cedar");
    let (schema, policies) = hypgm_authz::load_schema_and_policies(DEFAULT_SCHEMA, DEFAULT_POLICIES)
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

impl Default for AppState<tower_sessions::MemoryStore> {
    fn default() -> Self {
        Self::new(
            Arc::new(DefaultHelloWorldService),
            Arc::new(DefaultMockUserService),
            Arc::new(InMemoryAuthRepo::new()),
            default_authorizer(),
            tower_sessions::MemoryStore::default(),
            b"exemplum-default-secret-key-32bytes!".to_vec(),
        )
    }
}

impl AppState<tower_sessions::MemoryStore> {
    /// Constructs a fully working in-memory application state for tests and local development.
    pub fn in_memory(
        authorizer: Arc<Authorizer>,
        user_service: Arc<dyn UserService>,
        auth_repo: Arc<dyn AuthRepo>,
    ) -> Self {
        Self::new(
            Arc::new(DefaultHelloWorldService),
            user_service,
            auth_repo,
            authorizer,
            tower_sessions::MemoryStore::default(),
            b"exemplum-default-secret-key-32bytes!".to_vec(),
        )
    }
}

impl AppState<PostgresSessionStore> {
    /// Constructs a production application state backed by PostgreSQL.
    pub fn from_pool(authorizer: Arc<Authorizer>, pool: PgPool, jwt_secret: Vec<u8>) -> Self {
        let session_store = PostgresSessionStore::new_with_schema(pool.clone(), "exemplum");
        let auth_repo = Arc::new(PgAuthRepo::new(pool.clone()));
        let user_repo = Arc::new(PgUserRepo::new(pool));
        let user_service = Arc::new(DefaultUserService::new(user_repo));
        Self::new(
            Arc::new(DefaultHelloWorldService),
            user_service,
            auth_repo,
            authorizer,
            session_store,
            jwt_secret,
        )
    }
}
