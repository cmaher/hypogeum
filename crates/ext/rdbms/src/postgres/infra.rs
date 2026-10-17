use crate::contract::{DatabaseConfig, Result};
use hypgm_errors::MigrationError;
use sqlx::{AssertSqlSafe, PgConnection};

pub async fn apply_contracts(conn: &mut PgConnection, config: &DatabaseConfig) -> Result<()> {
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

    let owner = &config.owner_role;
    let tasks = &config.tasks_role;
    let mut revoke_roles = vec!["PUBLIC".to_string()];
    if !tasks.is_empty() {
        revoke_roles.push(tasks.clone());
    }
    for sc in &config.service_contracts {
        if !sc.service_role.is_empty() && !revoke_roles.contains(&sc.service_role) {
            revoke_roles.push(sc.service_role.clone());
        }
    }
    let revoke_list = revoke_roles.join(", ");

    let grant_tasks = if !tasks.is_empty() {
        format!(
            "GRANT USAGE ON SCHEMA apalis TO {tasks};\
             GRANT SELECT, INSERT, UPDATE, DELETE ON apalis.jobs, apalis.workers TO {tasks};\
             GRANT USAGE ON TYPE apalis.hstore TO {tasks};\
             GRANT EXECUTE ON FUNCTION apalis.get_jobs(text, text, integer) TO {tasks};\
             GRANT EXECUTE ON FUNCTION apalis.schema_version() TO {tasks};"
        )
    } else {
        String::new()
    };

    let sql = format!(
        "CREATE SCHEMA IF NOT EXISTS infra AUTHORIZATION {owner};\
         REVOKE ALL ON SCHEMA infra FROM PUBLIC;\
         REVOKE ALL ON SCHEMA apalis FROM {revoke_list};\
         REVOKE ALL ON ALL TABLES IN SCHEMA apalis FROM {revoke_list};\
         REVOKE ALL ON ALL SEQUENCES IN SCHEMA apalis FROM {revoke_list};\
         REVOKE ALL ON ALL FUNCTIONS IN SCHEMA apalis FROM {revoke_list};\
         REVOKE ALL ON TYPE apalis.hstore FROM {revoke_list};\
         CREATE OR REPLACE FUNCTION apalis.schema_version() RETURNS bigint \
         LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp \
         AS $$ SELECT 20260902075949::bigint $$;\
         REVOKE ALL ON FUNCTION apalis.schema_version() FROM PUBLIC;\
         {grant_tasks}"
    );

    sqlx::raw_sql(AssertSqlSafe(sql))
        .execute(&mut *conn)
        .await
        .map_err(MigrationError::database)?;

    Ok(())
}
