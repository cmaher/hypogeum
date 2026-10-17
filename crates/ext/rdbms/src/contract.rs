//! Service contracts and migration configurations.
use hypgm_errors::MigrationError;
use sea_orm_migration::MigratorTrait;

pub type Result<T> = std::result::Result<T, MigrationError>;

#[allow(dead_code)]
pub(crate) fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(MigrationError::InvalidState(message.into()))
    }
}

#[async_trait::async_trait]
pub trait DomainMigrator: Send + Sync {
    async fn migrate(&self, db: &sea_orm::DatabaseConnection) -> Result<()>;
}

pub struct SeaOrmAdapter<M: MigratorTrait + 'static + Sync>(std::marker::PhantomData<M>);

impl<M: MigratorTrait + 'static + Sync> Default for SeaOrmAdapter<M> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<M: MigratorTrait + 'static + Sync> SeaOrmAdapter<M> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }

    pub fn from_sea_orm() -> Box<dyn DomainMigrator> {
        Box::new(Self::new())
    }
}

#[async_trait::async_trait]
impl<M: MigratorTrait + 'static + Sync> DomainMigrator for SeaOrmAdapter<M> {
    async fn migrate(&self, db: &sea_orm::DatabaseConnection) -> Result<()> {
        M::up(db, None)
            .await
            .map_err(|e| MigrationError::InvalidState(e.to_string()))
    }
}

impl dyn DomainMigrator {
    pub fn from_sea_orm<M: MigratorTrait + 'static + Sync>() -> Box<dyn DomainMigrator> {
        Box::new(SeaOrmAdapter::<M>::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceContract {
    pub schema: String,
    pub service_role: String,
    pub version: i64,
    pub migration_version: Option<String>,
    pub table: String,
}

impl ServiceContract {
    pub fn new(
        schema: impl Into<String>,
        service_role: impl Into<String>,
        version: i64,
        migration_version: Option<impl Into<String>>,
        table: impl Into<String>,
    ) -> Self {
        Self {
            schema: schema.into(),
            service_role: service_role.into(),
            version,
            migration_version: migration_version.map(|v| v.into()),
            table: table.into(),
        }
    }
}

#[derive(Default)]
pub struct DatabaseConfig {
    pub migrator_role: String,
    pub owner_role: String,
    pub tasks_role: String,
    pub app_schemas: Vec<String>,
    pub domain_migrators: Vec<Box<dyn DomainMigrator>>,
    pub service_contracts: Vec<ServiceContract>,
}

impl DatabaseConfig {
    pub fn new(
        migrator_role: impl Into<String>,
        owner_role: impl Into<String>,
        tasks_role: impl Into<String>,
    ) -> Self {
        Self {
            migrator_role: migrator_role.into(),
            owner_role: owner_role.into(),
            tasks_role: tasks_role.into(),
            app_schemas: Vec::new(),
            domain_migrators: Vec::new(),
            service_contracts: Vec::new(),
        }
    }

    pub fn with_app_schema(mut self, schema: impl Into<String>) -> Self {
        self.app_schemas.push(schema.into());
        self
    }

    pub fn with_domain_migrator(mut self, migrator: Box<dyn DomainMigrator>) -> Self {
        self.domain_migrators.push(migrator);
        self
    }

    pub fn with_service_contract(mut self, contract: ServiceContract) -> Self {
        self.service_contracts.push(contract);
        self
    }
}
