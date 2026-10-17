//! Read-only administrative checks, separate from applying migrations.
use crate::{DatabaseConfig, Result, apalis, require};
use hypgm_errors::MigrationError;
use sqlx::{
    PgConnection,
    migrate::{Migrate, Migrator},
};

pub(crate) async fn connection(
    connection: &mut PgConnection,
    config: &DatabaseConfig,
) -> Result<()> {
    apalis::verify(connection).await?;

    for contract in &config.service_contracts {
        let version_query = format!("SELECT {}.schema_version()", contract.schema);
        let version: i64 = sqlx::query_scalar(&version_query)
            .fetch_one(&mut *connection)
            .await
            .map_err(MigrationError::database)?;
        require(
            version == contract.version,
            &format!("{} schema contract drift", contract.schema),
        )?;

        if let Some(ref mig_ver) = contract.migration_version {
            let history_query = format!(
                "SELECT version FROM {}.seaql_migrations WHERE version = $1",
                contract.schema
            );
            let migration_recorded: Option<String> = sqlx::query_scalar(&history_query)
                .bind(mig_ver)
                .fetch_optional(&mut *connection)
                .await
                .map_err(MigrationError::database)?;
            require(
                migration_recorded.is_some(),
                &format!("{} migration history drift", contract.schema),
            )?;
        }

        let sample_table_qual = if contract.table.contains('.') {
            contract.table.clone()
        } else {
            format!("{}.{}", contract.schema, contract.table)
        };
        let seaql_migrations = format!("{}.seaql_migrations", contract.schema);

        let service_query = format!(
            "SELECT \
             has_schema_privilege($1, $2, 'CREATE') OR has_schema_privilege($1, 'public', 'CREATE') \
             OR has_schema_privilege($1, 'infra', 'CREATE') \
             OR has_database_privilege($1, current_database(), 'CREATE,TEMPORARY') \
             OR has_table_privilege($1, 'apalis.jobs', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') \
             OR has_table_privilege($1, 'apalis._sqlx_migrations', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') \
             OR has_table_privilege($1, '{seaql_migrations}', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') \
             OR EXISTS(SELECT FROM pg_catalog.pg_auth_members m JOIN pg_catalog.pg_roles r ON r.oid=m.member WHERE r.rolname=$1)"
        );
        let unsafe_service: bool = sqlx::query_scalar(&service_query)
            .bind(&contract.service_role)
            .bind(&contract.schema)
            .fetch_one(&mut *connection)
            .await
            .map_err(MigrationError::database)?;
        require(!unsafe_service, "Runtime privilege boundary drift")?;

        let tasks_query = format!(
            "SELECT \
             has_schema_privilege($1, 'apalis', 'CREATE') OR has_schema_privilege($1, 'public', 'CREATE') \
             OR has_schema_privilege($1, 'infra', 'CREATE') \
             OR has_database_privilege($1, current_database(), 'CREATE,TEMPORARY') \
             OR has_table_privilege($1, $2, 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') \
             OR has_table_privilege($1, 'apalis._sqlx_migrations', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') \
             OR has_table_privilege($1, '{seaql_migrations}', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') \
             OR EXISTS(SELECT FROM pg_catalog.pg_auth_members m JOIN pg_catalog.pg_roles r ON r.oid=m.member WHERE r.rolname=$1)"
        );
        let unsafe_tasks: bool = sqlx::query_scalar(&tasks_query)
            .bind(&config.tasks_role)
            .bind(&sample_table_qual)
            .fetch_one(&mut *connection)
            .await
            .map_err(MigrationError::database)?;
        require(!unsafe_tasks, "Runtime privilege boundary drift")?;
    }

    if config.service_contracts.is_empty() && !config.tasks_role.is_empty() {
        let tasks_query = "SELECT \
             has_schema_privilege($1, 'apalis', 'CREATE') OR has_schema_privilege($1, 'public', 'CREATE') \
             OR has_schema_privilege($1, 'infra', 'CREATE') \
             OR has_database_privilege($1, current_database(), 'CREATE,TEMPORARY') \
             OR has_table_privilege($1, 'apalis._sqlx_migrations', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') \
             OR EXISTS(SELECT FROM pg_catalog.pg_auth_members m JOIN pg_catalog.pg_roles r ON r.oid=m.member WHERE r.rolname=$1)";
        let unsafe_tasks: bool = sqlx::query_scalar(tasks_query)
            .bind(&config.tasks_role)
            .fetch_one(&mut *connection)
            .await
            .map_err(MigrationError::database)?;
        require(!unsafe_tasks, "Runtime privilege boundary drift")?;
    }

    Ok(())
}

pub(crate) async fn check_history(
    connection: &mut PgConnection,
    table: &str,
    history: &Migrator,
) -> Result<()> {
    require(
        connection
            .dirty_version(table)
            .await
            .map_err(MigrationError::database)?
            .is_none(),
        "Incomplete migration history",
    )?;
    let applied = connection
        .list_applied_migrations(table)
        .await
        .map_err(MigrationError::database)?;
    let expected: Vec<_> = history
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .collect();
    require(
        applied.len() == expected.len(),
        "Incomplete migration history",
    )?;
    for (actual, expected) in applied.iter().zip(expected) {
        require(
            actual.version == expected.version && actual.checksum == expected.checksum,
            "Migration checksum/state mismatch",
        )?;
    }
    Ok(())
}
