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

    pub fn validate(&self) -> Result<()> {
        if !self.migrator_role.is_empty() {
            validate_identifier(&self.migrator_role)?;
        }
        if !self.owner_role.is_empty() {
            validate_identifier(&self.owner_role)?;
        }
        if !self.tasks_role.is_empty() {
            validate_identifier(&self.tasks_role)?;
        }
        for schema in &self.app_schemas {
            validate_identifier(schema)?;
        }
        for contract in &self.service_contracts {
            validate_identifier(&contract.schema)?;
            if !contract.service_role.is_empty() {
                validate_identifier(&contract.service_role)?;
            }
            for part in contract.table.split('.') {
                validate_identifier(part)?;
            }
        }
        Ok(())
    }
}

pub fn validate_identifier(ident: &str) -> Result<()> {
    let mut chars = ident.chars();
    let first = chars
        .next()
        .ok_or_else(|| MigrationError::InvalidState("SQL identifier cannot be empty".into()))?;
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(MigrationError::InvalidState(format!(
            "Invalid start character in SQL identifier: {ident:?}"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(MigrationError::InvalidState(format!(
            "Invalid character in SQL identifier: {ident:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_identifier_valid() {
        assert!(validate_identifier("users").is_ok());
        assert!(validate_identifier("app_schema_1").is_ok());
        assert!(validate_identifier("_system").is_ok());
    }

    #[test]
    fn test_validate_identifier_invalid() {
        assert!(validate_identifier("").is_err());
        assert!(validate_identifier("123abc").is_err());
        assert!(validate_identifier("bad-name").is_err());
        assert!(validate_identifier("user;drop table users;").is_err());
        assert!(validate_identifier("spaced name").is_err());
    }

    #[test]
    fn test_database_config_validate() {
        let valid = DatabaseConfig::new("my_migrator", "my_owner", "my_tasks")
            .with_app_schema("my_app")
            .with_service_contract(ServiceContract::new(
                "my_app",
                "my_service",
                1,
                Option::<String>::None,
                "users",
            ));
        assert!(valid.validate().is_ok());

        let invalid = DatabaseConfig::new("bad-migrator", "owner", "tasks");
        assert!(invalid.validate().is_err());
    }
}
