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

async fn owner(url: &str) -> Result<PgConnection> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == "hypogeum_migrator",
        "Use dedicated hypogeum_migrator credentials",
    )?;
    // Startup settings select the DDL role and place unchanged upstream objects
    // in their own schema. Our migration ledger and application DDL are qualified.
    let options = options.options([
        ("role", "hypogeum_owner"),
        ("statement_timeout", "30s"),
        ("search_path", "apalis,pg_catalog"),
    ]);
    PgConnection::connect_with(&options)
        .await
        .map_err(MigrationError::database)
}

async fn owner_pool(url: &str) -> Result<sqlx::PgPool> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == "hypogeum_migrator",
        "Use dedicated hypogeum_migrator credentials",
    )?;
    let options = options.options([
        ("role", "hypogeum_owner"),
        ("statement_timeout", "30s"),
        ("search_path", "hypogeum,pg_catalog"),
    ]);
    sqlx::PgPool::connect_with(options)
        .await
        .map_err(MigrationError::database)
}

pub async fn migrate(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
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
    infra::apply_contracts(&mut connection).await?;
    sqlx::raw_sql(
        "CREATE SCHEMA IF NOT EXISTS hypogeum AUTHORIZATION hypogeum_owner;\
         REVOKE ALL ON SCHEMA hypogeum FROM PUBLIC;",
    )
    .execute(&mut connection)
    .await
    .map_err(MigrationError::database)?;

    // Domain SeaORM migrators
    let pool = owner_pool(url).await?;
    let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
    let sea_result = hypgm_dmn_users::pg::migrations::Migrator::up(&db, None).await;
    pool.close().await;

    let _ = sqlx::query("SELECT pg_catalog.pg_advisory_unlock($1)")
        .bind(ADMIN_LOCK)
        .execute(&mut connection)
        .await;
    connection.close().await.map_err(MigrationError::database)?;

    sea_result.map_err(|e| MigrationError::InvalidState(e.to_string()))?;

    Ok(())
}

pub async fn verify(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
    verify::connection(&mut connection).await
}

pub async fn inspect(url: &str) -> Result<serde_json::Value> {
    let mut connection = owner(url).await?;
    let tables = sqlx::query("SELECT schemaname,tablename FROM pg_catalog.pg_tables WHERE schemaname IN ('hypogeum','infra','apalis') ORDER BY schemaname,tablename")
        .fetch_all(&mut connection).await.map_err(MigrationError::database)?;
    Ok(
        serde_json::json!({"tables": tables.iter().map(|r| serde_json::json!({"schema": r.get::<String,_>("schemaname"), "table": r.get::<String,_>("tablename")})).collect::<Vec<_>>()}),
    )
}
