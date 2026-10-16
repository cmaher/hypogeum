//! Explicit SQLx and SeaORM migrations; runtime processes never migrate.
mod apalis;
pub mod infra;
mod verify;
use errors::MigrationError;
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

async fn owner(url: &str) -> Result<PgConnection> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == "foundation_migrator",
        "Use dedicated foundation_migrator credentials",
    )?;
    // Startup settings select the DDL role and place unchanged upstream objects
    // in their own schema. Our migration ledger and application DDL are qualified.
    let options = options.options([
        ("role", "foundation_owner"),
        ("statement_timeout", "30s"),
        ("search_path", "fndapp,apalis,pg_catalog"),
    ]);
    PgConnection::connect_with(&options)
        .await
        .map_err(MigrationError::database)
}

async fn owner_pool(url: &str) -> Result<sqlx::PgPool> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == "foundation_migrator",
        "Use dedicated foundation_migrator credentials",
    )?;
    let options = options.options([
        ("role", "foundation_owner"),
        ("statement_timeout", "30s"),
        ("search_path", "fndapp,apalis,pg_catalog"),
    ]);
    sqlx::PgPool::connect_with(options)
        .await
        .map_err(MigrationError::database)
}

pub async fn migrate(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
    // Each SQLx migrator locks and validates its own unchanged migration history.
    apalis::migrations()
        .run(&mut connection)
        .await
        .map_err(MigrationError::database)?;
    infra::apply_contracts(&mut connection).await?;
    sqlx::raw_sql(
        "CREATE SCHEMA IF NOT EXISTS fndapp AUTHORIZATION foundation_owner;\
         REVOKE ALL ON SCHEMA fndapp FROM PUBLIC;",
    )
    .execute(&mut connection)
    .await
    .map_err(MigrationError::database)?;
    connection.close().await.map_err(MigrationError::database)?;

    // Domain SeaORM migrators
    let pool = owner_pool(url).await?;
    let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
    users::pg::migrations::Migrator::up(&db, None)
        .await
        .map_err(|e| MigrationError::InvalidState(e.to_string()))?;
    pool.close().await;

    Ok(())
}

pub async fn verify(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
    verify::connection(&mut connection).await
}

pub async fn inspect(url: &str) -> Result<serde_json::Value> {
    let mut connection = owner(url).await?;
    let tables = sqlx::query("SELECT schemaname,tablename FROM pg_catalog.pg_tables WHERE schemaname IN ('fndapp','infra','apalis') ORDER BY schemaname,tablename")
        .fetch_all(&mut connection).await.map_err(MigrationError::database)?;
    Ok(
        serde_json::json!({"tables": tables.iter().map(|r| serde_json::json!({"schema": r.get::<String,_>("schemaname"), "table": r.get::<String,_>("tablename")})).collect::<Vec<_>>()}),
    )
}
