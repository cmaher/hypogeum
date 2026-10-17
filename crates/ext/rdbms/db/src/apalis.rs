use crate::{Result, require, verify::check_history};
use apalis_postgres::PostgresStorage;
use errors::MigrationError;
use sqlx::{PgConnection, migrate::Migrator};

pub(crate) fn migrations() -> Migrator {
    let mut migrations = PostgresStorage::migrations();
    migrations.create_schema("apalis");
    migrations.dangerous_set_table_name("apalis._sqlx_migrations");
    migrations
}

pub(crate) async fn verify(connection: &mut PgConnection) -> Result<()> {
    check_history(connection, "apalis._sqlx_migrations", &migrations()).await?;
    let extensions: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_catalog.pg_extension e JOIN pg_catalog.pg_namespace n ON n.oid=e.extnamespace WHERE e.extname IN ('pgcrypto','hstore') AND n.nspname='apalis'")
        .fetch_one(&mut *connection).await.map_err(MigrationError::database)?;
    require(extensions == 2, "Apalis extension namespace drift")?;
    let version: i64 = sqlx::query_scalar("SELECT apalis.schema_version()")
        .fetch_one(&mut *connection)
        .await
        .map_err(MigrationError::database)?;
    require(version == 20260902075949, "Apalis schema contract drift")
}
