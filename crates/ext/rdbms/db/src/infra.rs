use crate::Result;
use errors::MigrationError;
use sqlx::{Executor, PgConnection};

pub async fn apply_contracts(conn: &mut PgConnection) -> Result<()> {
    let already_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_catalog.pg_proc p
            JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
            WHERE n.nspname = 'apalis' AND p.proname = 'schema_version'
        )",
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(MigrationError::database)?;

    if already_applied {
        return Ok(());
    }

    conn.execute(
        "CREATE SCHEMA IF NOT EXISTS infra AUTHORIZATION hypogeum_owner;\
         REVOKE ALL ON SCHEMA infra FROM PUBLIC;\
         REVOKE ALL ON SCHEMA apalis FROM PUBLIC, hypogeum_users, hypogeum_tasks;\
         REVOKE ALL ON ALL TABLES IN SCHEMA apalis FROM PUBLIC, hypogeum_users, hypogeum_tasks;\
         REVOKE ALL ON ALL SEQUENCES IN SCHEMA apalis FROM PUBLIC, hypogeum_users, hypogeum_tasks;\
         REVOKE ALL ON ALL FUNCTIONS IN SCHEMA apalis FROM PUBLIC, hypogeum_users, hypogeum_tasks;\
         REVOKE ALL ON TYPE apalis.hstore FROM PUBLIC, hypogeum_users, hypogeum_tasks;\
         GRANT USAGE ON SCHEMA apalis TO hypogeum_tasks;\
         GRANT SELECT, INSERT, UPDATE, DELETE ON apalis.jobs, apalis.workers TO hypogeum_tasks;\
         GRANT USAGE ON TYPE apalis.hstore TO hypogeum_tasks;\
         GRANT EXECUTE ON FUNCTION apalis.get_jobs(text, text, integer) TO hypogeum_tasks;\
         CREATE OR REPLACE FUNCTION apalis.schema_version() RETURNS bigint \
         LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp \
         AS $$ SELECT 20260902075949::bigint $$;\
         REVOKE ALL ON FUNCTION apalis.schema_version() FROM PUBLIC;\
         GRANT EXECUTE ON FUNCTION apalis.schema_version() TO hypogeum_tasks;",
    )
    .await
    .map_err(MigrationError::database)?;

    Ok(())
}
