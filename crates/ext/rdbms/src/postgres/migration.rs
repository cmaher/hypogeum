//! Postgres migration execution, verification, and schema inspection.
use crate::contract::{DatabaseConfig, Result, require};
use crate::postgres::{apalis, infra, verify as verify_checks};
use hypgm_errors::MigrationError;
use sqlx::{AssertSqlSafe, Connection, PgConnection, Row, postgres::PgConnectOptions};
use std::str::FromStr;

// Stable database administration lock, independent of project customization.
const ADMIN_LOCK: i64 = 0x4442_5f41_444d_494e;

pub async fn owner(url: &str, config: &DatabaseConfig) -> Result<PgConnection> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == config.migrator_role,
        &format!("Use dedicated {} credentials", config.migrator_role),
    )?;
    // Startup settings select the DDL role and place unchanged upstream objects
    // in their own schema. Our migration ledger and application DDL are qualified.
    let options = options.options([
        ("role", config.owner_role.as_str()),
        ("statement_timeout", "30s"),
        ("search_path", "apalis,pg_catalog"),
    ]);
    PgConnection::connect_with(&options)
        .await
        .map_err(MigrationError::database)
}

pub async fn owner_pool(url: &str, config: &DatabaseConfig) -> Result<sqlx::PgPool> {
    let options = PgConnectOptions::from_str(url).map_err(MigrationError::database)?;
    require(
        options.get_username() == config.migrator_role,
        &format!("Use dedicated {} credentials", config.migrator_role),
    )?;
    let search_path = if let Some(first_schema) = config.app_schemas.first() {
        format!("{first_schema},pg_catalog")
    } else {
        "apalis,pg_catalog".to_string()
    };
    let options = options.options([
        ("role", config.owner_role.as_str()),
        ("statement_timeout", "30s"),
        ("search_path", search_path.as_str()),
    ]);
    sqlx::PgPool::connect_with(options)
        .await
        .map_err(MigrationError::database)
}

pub async fn migrate(url: &str, config: &DatabaseConfig) -> Result<()> {
    let mut connection = owner(url, config).await?;
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
    infra::apply_contracts(&mut connection, config).await?;

    for schema in &config.app_schemas {
        let sql = format!(
            "CREATE SCHEMA IF NOT EXISTS {schema} AUTHORIZATION {};\
             REVOKE ALL ON SCHEMA {schema} FROM PUBLIC;",
            config.owner_role
        );
        sqlx::raw_sql(AssertSqlSafe(sql))
            .execute(&mut connection)
            .await
            .map_err(MigrationError::database)?;
    }

    // Domain SeaORM migrators
    if !config.domain_migrators.is_empty() {
        let pool = owner_pool(url, config).await?;
        let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        let mut sea_error = None;
        for migrator in &config.domain_migrators {
            if let Err(e) = migrator.migrate(&db).await {
                sea_error = Some(e);
                break;
            }
        }
        pool.close().await;

        if let Some(err) = sea_error {
            let _ = sqlx::query("SELECT pg_catalog.pg_advisory_unlock($1)")
                .bind(ADMIN_LOCK)
                .execute(&mut connection)
                .await;
            connection.close().await.map_err(MigrationError::database)?;
            return Err(err);
        }
    }

    let _ = sqlx::query("SELECT pg_catalog.pg_advisory_unlock($1)")
        .bind(ADMIN_LOCK)
        .execute(&mut connection)
        .await;
    connection.close().await.map_err(MigrationError::database)?;

    Ok(())
}

pub async fn verify(url: &str, config: &DatabaseConfig) -> Result<()> {
    let mut connection = owner(url, config).await?;
    verify_checks::connection(&mut connection, config).await
}

pub async fn inspect(url: &str, config: &DatabaseConfig) -> Result<serde_json::Value> {
    let mut connection = owner(url, config).await?;
    let mut schemas = vec!["infra".to_string(), "apalis".to_string()];
    for s in &config.app_schemas {
        if !schemas.contains(s) {
            schemas.push(s.clone());
        }
    }
    let tables: Vec<String> = sqlx::query(
        "SELECT schemaname || '.' || tablename FROM pg_catalog.pg_tables \
         WHERE schemaname = ANY($1) ORDER BY 1",
    )
    .bind(&schemas)
    .fetch_all(&mut connection)
    .await
    .map_err(MigrationError::database)?
    .into_iter()
    .map(|row| row.get(0))
    .collect();

    let functions: Vec<String> = sqlx::query(
        "SELECT n.nspname || '.' || p.proname FROM pg_catalog.pg_proc p \
         JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = ANY($1) ORDER BY 1",
    )
    .bind(&schemas)
    .fetch_all(&mut connection)
    .await
    .map_err(MigrationError::database)?
    .into_iter()
    .map(|row| row.get(0))
    .collect();

    let apalis_version: i64 = sqlx::query_scalar("SELECT apalis.schema_version()")
        .fetch_one(&mut connection)
        .await
        .map_err(MigrationError::database)?;

    let mut contracts_info = serde_json::Map::new();
    for sc in &config.service_contracts {
        let version_query = format!("SELECT {}.schema_version()", sc.schema);
        let version: std::result::Result<i64, _> = sqlx::query_scalar(AssertSqlSafe(version_query))
            .fetch_one(&mut connection)
            .await;
        let ver_val = match version {
            Ok(v) => serde_json::json!(v),
            Err(_) => serde_json::json!(null),
        };
        contracts_info.insert(
            sc.schema.clone(),
            serde_json::json!({
                "service_role": sc.service_role,
                "version": ver_val,
                "table": sc.table,
            }),
        );
    }

    Ok(serde_json::json!({
        "apalis_version": apalis_version,
        "schemas": schemas,
        "tables": tables,
        "functions": functions,
        "contracts": contracts_info,
    }))
}
