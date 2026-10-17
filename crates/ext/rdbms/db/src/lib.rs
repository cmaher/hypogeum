//! Explicit SQLx and SeaORM migrations; runtime processes never migrate.
mod apalis;
pub mod infra;
mod verify;
use hypgm_errors::MigrationError;
use sea_orm_migration::MigratorTrait;
use sqlx::{Connection, PgConnection, Row, postgres::PgConnectOptions};
use std::str::FromStr;

type Result<T> = std::result::Result<T, MigrationError>;

pub(crate) fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(MigrationError::InvalidState(message.into()))
    }
}

// Stable database administration lock, independent of project customization.
const ADMIN_LOCK: i64 = 0x4442_5f41_444d_494e;

#[async_trait::async_trait]
pub trait DomainMigrator: Send + Sync {
    async fn migrate(&self, db: &sea_orm::DatabaseConnection) -> Result<()>;
}

pub struct SeaOrmAdapter<M: MigratorTrait + 'static>(std::marker::PhantomData<M>);

impl<M: MigratorTrait + 'static> Default for SeaOrmAdapter<M> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<M: MigratorTrait + 'static> SeaOrmAdapter<M> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }

    pub fn from_sea_orm() -> Box<dyn DomainMigrator> {
        Box::new(Self::new())
    }
}

#[async_trait::async_trait]
impl<M: MigratorTrait + 'static> DomainMigrator for SeaOrmAdapter<M> {
    async fn migrate(&self, db: &sea_orm::DatabaseConnection) -> Result<()> {
        M::up(db, None)
            .await
            .map_err(|e| MigrationError::InvalidState(e.to_string()))
    }
}

impl dyn DomainMigrator {
    pub fn from_sea_orm<M: MigratorTrait + 'static>() -> Box<dyn DomainMigrator> {
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

pub struct DatabaseConfig {
    pub migrator_role: String,
    pub owner_role: String,
    pub tasks_role: String,
    pub app_schemas: Vec<String>,
    pub domain_migrators: Vec<Box<dyn DomainMigrator>>,
    pub service_contracts: Vec<ServiceContract>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            migrator_role: String::new(),
            owner_role: String::new(),
            tasks_role: String::new(),
            app_schemas: Vec::new(),
            domain_migrators: Vec::new(),
            service_contracts: Vec::new(),
        }
    }
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

async fn owner(url: &str, config: &DatabaseConfig) -> Result<PgConnection> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == config.migrator_role,
        &format!("Use dedicated {} credentials", config.migrator_role),
    )?;
    // Startup settings select the DDL role and place unchanged upstream objects
    // in their own schema. Our migration ledger and application DDL are qualified.
    let options = options.options([
        ("role", config.owner_role.as_str()),
        ("statement_timeout", "30s"),
        ("search_path", "apalis,pg_catalog"),
    ]);
    PgConnection::connect_with(&options)
        .await
        .map_err(MigrationError::database)
}

async fn owner_pool(url: &str, config: &DatabaseConfig) -> Result<sqlx::PgPool> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == config.migrator_role,
        &format!("Use dedicated {} credentials", config.migrator_role),
    )?;
    let search_path = if let Some(first_schema) = config.app_schemas.first() {
        format!("{first_schema},pg_catalog")
    } else {
        "apalis,pg_catalog".to_string()
    };
    let options = options.options([
        ("role", config.owner_role.as_str()),
        ("statement_timeout", "30s"),
        ("search_path", search_path.as_str()),
    ]);
    sqlx::PgPool::connect_with(options)
        .await
        .map_err(MigrationError::database)
}

pub async fn migrate(url: &str, config: &DatabaseConfig) -> Result<()> {
    let mut connection = owner(url, config).await?;
    sqlx::query("SELECT pg_catalog.pg_advisory_lock($1)")
        .bind(ADMIN_LOCK)
        .execute(&mut connection)
        .await
        .map_err(MigrationError::database)?;

    // Each SQLx migrator locks and validates its own unchanged migration history.
    apalis::migrations()
        .run(&mut connection)
        .await
        .map_err(MigrationError::database)?;
    infra::apply_contracts(&mut connection, config).await?;

    for schema in &config.app_schemas {
        let sql = format!(
            "CREATE SCHEMA IF NOT EXISTS {schema} AUTHORIZATION {};\
             REVOKE ALL ON SCHEMA {schema} FROM PUBLIC;",
            config.owner_role
        );
        sqlx::raw_sql(&sql)
            .execute(&mut connection)
            .await
            .map_err(MigrationError::database)?;
    }

    // Domain SeaORM migrators
    if !config.domain_migrators.is_empty() {
        let pool = owner_pool(url, config).await?;
        let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        let mut sea_error = None;
        for migrator in &config.domain_migrators {
            if let Err(e) = migrator.migrate(&db).await {
                sea_error = Some(e);
                break;
            }
        }
        pool.close().await;

        if let Some(err) = sea_error {
            let _ = sqlx::query("SELECT pg_catalog.pg_advisory_unlock($1)")
                .bind(ADMIN_LOCK)
                .execute(&mut connection)
                .await;
            connection.close().await.map_err(MigrationError::database)?;
            return Err(err);
        }
    }

    let _ = sqlx::query("SELECT pg_catalog.pg_advisory_unlock($1)")
        .bind(ADMIN_LOCK)
        .execute(&mut connection)
        .await;
    connection.close().await.map_err(MigrationError::database)?;

    Ok(())
}

pub async fn verify(url: &str, config: &DatabaseConfig) -> Result<()> {
    let mut connection = owner(url, config).await?;
    verify::connection(&mut connection, config).await
}

pub async fn inspect(url: &str, config: &DatabaseConfig) -> Result<serde_json::Value> {
    let mut connection = owner(url, config).await?;
    let mut schemas = vec!["infra".to_string(), "apalis".to_string()];
    for s in &config.app_schemas {
        if !schemas.contains(s) {
            schemas.push(s.clone());
        }
    }
    let in_list = schemas
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(",");
    let query_str = format!(
        "SELECT schemaname,tablename FROM pg_catalog.pg_tables WHERE schemaname IN ({in_list}) ORDER BY schemaname,tablename"
    );
    let tables = sqlx::query(&query_str)
        .fetch_all(&mut connection)
        .await
        .map_err(MigrationError::database)?;
    Ok(
        serde_json::json!({"tables": tables.iter().map(|r| serde_json::json!({"schema": r.get::<String,_>("schemaname"), "table": r.get::<String,_>("tablename")})).collect::<Vec<_>>()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyMigrator;
    #[async_trait::async_trait]
    impl MigratorTrait for DummyMigrator {
        fn migrations() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
            vec![]
        }
    }

    #[test]
    fn test_database_config_builder() {
        let config = DatabaseConfig::new("migrator", "owner", "tasks")
            .with_app_schema("app")
            .with_domain_migrator(DomainMigrator::from_sea_orm::<DummyMigrator>())
            .with_service_contract(ServiceContract::new(
                "app",
                "app_user",
                1,
                Some("m20260101_init"),
                "users",
            ));

        assert_eq!(config.migrator_role, "migrator");
        assert_eq!(config.owner_role, "owner");
        assert_eq!(config.tasks_role, "tasks");
        assert_eq!(config.app_schemas, vec!["app"]);
        assert_eq!(config.domain_migrators.len(), 1);
        assert_eq!(config.service_contracts.len(), 1);
        assert_eq!(config.service_contracts[0].table, "users");
    }
}
