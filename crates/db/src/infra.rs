use crate::Result;
use errors::MigrationError;
use sqlx::{Executor, PgConnection};

pub async fn apply_contracts(conn: &mut PgConnection) -> Result<()> {
    conn.execute(
        "CREATE SCHEMA IF NOT EXISTS infra AUTHORIZATION foundation_owner;\
         REVOKE ALL ON SCHEMA infra FROM PUBLIC;\
         REVOKE ALL ON SCHEMA apalis FROM PUBLIC, foundation_users, foundation_tasks;\
         REVOKE ALL ON ALL TABLES IN SCHEMA apalis FROM PUBLIC, foundation_users, foundation_tasks;\
         REVOKE ALL ON ALL SEQUENCES IN SCHEMA apalis FROM PUBLIC, foundation_users, foundation_tasks;\
         REVOKE ALL ON ALL FUNCTIONS IN SCHEMA apalis FROM PUBLIC, foundation_users, foundation_tasks;\
         REVOKE ALL ON TYPE apalis.hstore FROM PUBLIC, foundation_users, foundation_tasks;\
         GRANT USAGE ON SCHEMA apalis TO foundation_tasks;\
         GRANT SELECT, INSERT, UPDATE, DELETE ON apalis.jobs, apalis.workers TO foundation_tasks;\
         GRANT USAGE ON TYPE apalis.hstore TO foundation_tasks;\
         GRANT EXECUTE ON FUNCTION apalis.get_jobs(text, text, integer) TO foundation_tasks;\
         CREATE OR REPLACE FUNCTION apalis.schema_version() RETURNS bigint \
         LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp \
         AS $$ SELECT 20260902075949::bigint $$;\
         REVOKE ALL ON FUNCTION apalis.schema_version() FROM PUBLIC;\
         GRANT EXECUTE ON FUNCTION apalis.schema_version() TO foundation_tasks;",
    )
    .await
    .map_err(MigrationError::database)?;

    Ok(())
}
