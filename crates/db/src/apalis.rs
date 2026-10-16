use crate::{Result, check_history, require};
use apalis_postgres::PostgresStorage;
use errors::MigrationError;
use sqlx::PgConnection;

pub(crate) async fn migrate(connection: &mut PgConnection) -> Result<()> {
    sqlx::raw_sql("CREATE SCHEMA IF NOT EXISTS apalis AUTHORIZATION foundation_owner;
        REVOKE ALL ON SCHEMA apalis FROM PUBLIC; SET search_path=apalis,pg_catalog;")
        .execute(&mut *connection).await.map_err(MigrationError::database)?;
    // Upstream SQL is applied unchanged, with a separate schema and ledger.
    PostgresStorage::migrations().run(&mut *connection).await.map_err(MigrationError::database)?;
    sqlx::raw_sql("BEGIN;
        REVOKE ALL ON SCHEMA apalis FROM PUBLIC,foundation_users,foundation_tasks;
        REVOKE ALL ON ALL TABLES IN SCHEMA apalis FROM PUBLIC,foundation_users,foundation_tasks;
        REVOKE ALL ON ALL SEQUENCES IN SCHEMA apalis FROM PUBLIC,foundation_users,foundation_tasks;
        REVOKE ALL ON ALL FUNCTIONS IN SCHEMA apalis FROM PUBLIC,foundation_users,foundation_tasks;
        REVOKE ALL ON TYPE apalis.hstore FROM PUBLIC,foundation_users,foundation_tasks;
        GRANT USAGE ON SCHEMA apalis TO foundation_tasks;
        GRANT SELECT,INSERT,UPDATE,DELETE ON apalis.jobs,apalis.workers TO foundation_tasks;
        GRANT USAGE ON TYPE apalis.hstore TO foundation_tasks;
        GRANT EXECUTE ON FUNCTION apalis.get_jobs(text,text,integer) TO foundation_tasks;
        CREATE OR REPLACE FUNCTION apalis.schema_version() RETURNS bigint
        LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
          SELECT CASE WHEN count(*)=23 AND bool_and(success) AND max(version)=20260902075949 THEN 20260902075949::bigint ELSE 0::bigint END FROM apalis._sqlx_migrations
        $$;
        REVOKE ALL ON FUNCTION apalis.schema_version() FROM PUBLIC;
        GRANT EXECUTE ON FUNCTION apalis.schema_version() TO foundation_tasks;
        COMMIT;")
        .execute(&mut *connection).await.map_err(MigrationError::database)?;
    Ok(())
}

pub(crate) async fn verify(connection: &mut PgConnection) -> Result<()> {
    check_history(connection, "apalis", &PostgresStorage::migrations()).await?;
    let extensions: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_catalog.pg_extension e JOIN pg_catalog.pg_namespace n ON n.oid=e.extnamespace WHERE e.extname IN ('pgcrypto','hstore') AND n.nspname='apalis'")
        .fetch_one(&mut *connection).await.map_err(MigrationError::database)?;
    require(extensions == 2, "Apalis extension namespace drift")?;
    let version: i64 = sqlx::query_scalar("SELECT apalis.schema_version()")
        .fetch_one(&mut *connection).await.map_err(MigrationError::database)?;
    require(version == 20260902075949, "Apalis schema contract drift")
}
