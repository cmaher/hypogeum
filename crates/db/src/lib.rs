//! Explicit SQLx migrations; runtime processes never migrate.
mod apalis;
mod verify;
use errors::MigrationError;
use sqlx::{Connection, PgConnection, Row, migrate::Migrator, postgres::PgConnectOptions};
use std::str::FromStr;

type Result<T> = std::result::Result<T, MigrationError>;

fn migrations() -> Migrator {
    let mut migrations = sqlx::migrate!("./resources/migrations");
    migrations.create_schema("infra");
    migrations.dangerous_set_table_name("infra._sqlx_migrations");
    migrations
}

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
        ("search_path", "apalis,pg_catalog"),
    ]);
    PgConnection::connect_with(&options)
        .await
        .map_err(MigrationError::database)
}

pub async fn migrate(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
    // Each SQLx migrator locks and validates its own unchanged migration history.
    // Application migrations run last so they can grant access to upstream objects.
    apalis::migrations()
        .run(&mut connection)
        .await
        .map_err(MigrationError::database)?;
    migrations()
        .run(&mut connection)
        .await
        .map_err(MigrationError::database)?;
    connection.close().await.map_err(MigrationError::database)
}

pub async fn verify(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
    verify::connection(&mut connection).await
}

pub async fn inspect(url: &str) -> Result<serde_json::Value> {
    let mut connection = owner(url).await?;
    let tables = sqlx::query("SELECT schemaname,tablename FROM pg_catalog.pg_tables WHERE schemaname IN ('app','infra','apalis') ORDER BY schemaname,tablename")
        .fetch_all(&mut connection).await.map_err(MigrationError::database)?;
    Ok(
        serde_json::json!({"tables": tables.iter().map(|r| serde_json::json!({"schema": r.get::<String,_>("schemaname"), "table": r.get::<String,_>("tablename")})).collect::<Vec<_>>()}),
    )
}
