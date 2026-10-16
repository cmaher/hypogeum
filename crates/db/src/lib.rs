//! Explicit administration using a dedicated connection; runtime processes never migrate.
mod apalis;
use errors::MigrationError;
use sqlx::{Connection, PgConnection, Row, migrate::Migrator};

type Result<T> = std::result::Result<T, MigrationError>;
static MIGRATOR: Migrator = sqlx::migrate!("./resources/migrations");
const ADMIN_LOCK: i64 = 0x464e_445f_4442;

pub(crate) fn require(condition: bool, message: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(MigrationError::InvalidState(message.into())) }
}

async fn owner(url: &str) -> Result<PgConnection> {
    let mut connection = PgConnection::connect(url).await.map_err(MigrationError::database)?;
    let user: String = sqlx::query_scalar("SELECT session_user::text").fetch_one(&mut connection).await.map_err(MigrationError::database)?;
    require(user == "foundation_migrator", "Use dedicated foundation_migrator credentials")?;
    sqlx::raw_sql("SET ROLE foundation_owner; SET statement_timeout='30s'; SET search_path=pg_catalog;")
        .execute(&mut connection).await.map_err(MigrationError::database)?;
    Ok(connection)
}

pub async fn migrate(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
    // Session lock lifetime is exactly this dedicated connection, including errors.
    sqlx::query("SELECT pg_catalog.pg_advisory_lock($1)").bind(ADMIN_LOCK)
        .execute(&mut connection).await.map_err(MigrationError::database)?;
    sqlx::raw_sql("ALTER DEFAULT PRIVILEGES REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
        ALTER DEFAULT PRIVILEGES REVOKE USAGE ON TYPES FROM PUBLIC;
        CREATE SCHEMA IF NOT EXISTS infra AUTHORIZATION foundation_owner;
        REVOKE ALL ON SCHEMA infra FROM PUBLIC;
        SET search_path=infra,pg_catalog;")
        .execute(&mut connection).await.map_err(MigrationError::database)?;
    MIGRATOR.run(&mut connection).await.map_err(MigrationError::database)?;
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION app.schema_version() RETURNS bigint
        LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
          SELECT CASE WHEN count(*)=1 AND bool_and(success) AND max(version)=1 THEN 1::bigint ELSE 0::bigint END FROM infra._sqlx_migrations
        $$;
        REVOKE ALL ON FUNCTION app.schema_version() FROM PUBLIC;
        GRANT EXECUTE ON FUNCTION app.schema_version() TO foundation_users;")
        .execute(&mut connection).await.map_err(MigrationError::database)?;
    apalis::migrate(&mut connection).await?;
    verify_connection(&mut connection).await?;
    connection.close().await.map_err(MigrationError::database)
}

pub async fn verify(url: &str) -> Result<()> {
    let mut connection = owner(url).await?;
    verify_connection(&mut connection).await
}

async fn verify_connection(connection: &mut PgConnection) -> Result<()> {
    check_history(connection, "infra", &MIGRATOR).await?;
    apalis::verify(connection).await?;
    let version: i64 = sqlx::query_scalar("SELECT app.schema_version()")
        .fetch_one(&mut *connection).await.map_err(MigrationError::database)?;
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

pub(crate) async fn check_history(connection: &mut PgConnection, schema: &str, history: &Migrator) -> Result<()> {
    // Schema comes only from fixed internal callers, never user input.
    let query = match schema {
        "infra" => "SELECT version,checksum,success FROM infra._sqlx_migrations ORDER BY version",
        "apalis" => "SELECT version,checksum,success FROM apalis._sqlx_migrations ORDER BY version",
        _ => return Err(MigrationError::InvalidState("Unknown migration schema".into())),
    };
    let rows = sqlx::query(query).fetch_all(&mut *connection).await.map_err(MigrationError::database)?;
    require(rows.len() == history.iter().count(), "Incomplete migration history")?;
    for (row, migration) in rows.iter().zip(history.iter()) {
        require(row.get::<i64,_>("version") == migration.version && row.get::<Vec<u8>,_>("checksum") == migration.checksum.as_ref() && row.get::<bool,_>("success"), "Migration checksum/state mismatch")?;
    }
    Ok(())
}

pub async fn inspect(url: &str) -> Result<serde_json::Value> {
    let mut connection = owner(url).await?;
    let tables = sqlx::query("SELECT schemaname,tablename FROM pg_catalog.pg_tables WHERE schemaname IN ('app','infra','apalis') ORDER BY schemaname,tablename")
        .fetch_all(&mut connection).await.map_err(MigrationError::database)?;
    Ok(serde_json::json!({"tables": tables.iter().map(|r| serde_json::json!({"schema": r.get::<String,_>("schemaname"), "table": r.get::<String,_>("tablename")})).collect::<Vec<_>>()}))
}
