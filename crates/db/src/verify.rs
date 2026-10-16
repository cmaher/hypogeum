//! Read-only administrative checks, separate from applying migrations.
use crate::{Result, apalis, migrations, require};
use errors::MigrationError;
use sqlx::{
    PgConnection,
    migrate::{Migrate, Migrator},
};

pub(crate) async fn connection(connection: &mut PgConnection) -> Result<()> {
    check_history(connection, "infra._sqlx_migrations", &migrations()).await?;
    apalis::verify(connection).await?;
    let version: i64 = sqlx::query_scalar("SELECT app.schema_version()")
        .fetch_one(&mut *connection)
        .await
        .map_err(MigrationError::database)?;
    require(version == 1, "Users schema contract drift")?;
    for (role, schema, other_table) in [
        ("foundation_users", "app", "apalis.jobs"),
        ("foundation_tasks", "apalis", "app.users"),
    ] {
        let unsafe_access: bool = sqlx::query_scalar("SELECT
            has_schema_privilege($1,$2,'CREATE') OR has_schema_privilege($1,'public','CREATE')
            OR has_schema_privilege($1,'infra','CREATE')
            OR has_database_privilege($1,current_database(),'CREATE,TEMPORARY')
            OR has_table_privilege($1,$3,'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
            OR has_table_privilege($1,'infra._sqlx_migrations','SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
            OR has_table_privilege($1,'apalis._sqlx_migrations','SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
            OR EXISTS(SELECT FROM pg_catalog.pg_auth_members m JOIN pg_catalog.pg_roles r ON r.oid=m.member WHERE r.rolname=$1)")
            .bind(role).bind(schema).bind(other_table).fetch_one(&mut *connection).await.map_err(MigrationError::database)?;
        require(!unsafe_access, "Runtime privilege boundary drift")?;
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
