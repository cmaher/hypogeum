use exemplum_dmn_users::{
    models::PageRequest,
    repo::{PgUserRepo, UserRepo},
};
use hypgm_errors::RepoError;
use hypgm_test_support::TestDatabase;
use sea_orm_migration::MigratorTrait;
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use uuid::Uuid;

fn exemplum_config() -> hypgm_ext_db::DatabaseConfig {
    hypgm_ext_db::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
        .with_app_schema("exemplum")
        .with_domain_migrator(hypgm_ext_db::DomainMigrator::from_sea_orm::<
            exemplum_dmn_users::pg::migrations::Migrator,
        >())
        .with_service_contract(hypgm_ext_db::ServiceContract::new(
            "exemplum",
            "exemplum_users",
            1,
            Some("m20260101_000001_create_users"),
            "users",
        ))
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn existing_user_survives_migration_upgrade(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    use std::str::FromStr;
    let db = TestDatabase::provision(options).await;
    let owner = PgConnectOptions::from_str(&db.exemplum_migration_url)
        .unwrap()
        .options([
            ("role", "exemplum_owner"),
            ("search_path", "exemplum,pg_catalog"),
        ]);
    let migrator_pool = sqlx::PgPool::connect_with(owner).await.unwrap();
    let sea_db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(migrator_pool.clone());
    exemplum_dmn_users::pg::migrations::Migrator::up(&sea_db, Some(1))
        .await
        .unwrap();
    migrator_pool.close().await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = PgUserRepo::new(pool.clone());
    let before = repo.create(Uuid::new_v4(), "Existing User").await.unwrap();
    db.migrate_with(&exemplum_config()).await;
    assert_eq!(repo.get(before.id).await.unwrap(), before);
    exemplum_dmn_users::repo::check_ready(&pool).await.unwrap();
    hypgm_ext_db::verify(&db.exemplum_migration_url, &exemplum_config())
        .await
        .unwrap();
    pool.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn update_name_never_moves_database_timestamps_backwards(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    let db = TestDatabase::provision(options).await;
    db.migrate_with(&exemplum_config()).await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = PgUserRepo::new(pool.clone());
    let created = repo.create(Uuid::new_v4(), "Clock Test").await.unwrap();

    // Simulate a stored timestamp ahead of the current clock without changing clocks.
    let previous: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "UPDATE exemplum.users SET updated_at = statement_timestamp() + INTERVAL '1 day' \
         WHERE id = $1 RETURNING updated_at",
    )
    .bind(created.id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let updated = repo
        .update_name(created.id, "Renamed Clock Test")
        .await
        .unwrap();
    assert_eq!(updated.created_at, created.created_at);
    assert!(updated.updated_at >= previous);
    assert_eq!(repo.get(created.id).await.unwrap(), updated);
    pool.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn user_repository_connects_and_persists_under_runtime_privileges(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    let db = TestDatabase::provision(options).await;
    let config = exemplum_config();
    let migration_url = db.exemplum_migration_url.clone();
    // Concurrent runners must serialize and remain repeatable.
    let (first, second) = tokio::join!(
        hypgm_ext_db::migrate(&migration_url, &config),
        hypgm_ext_db::migrate(&migration_url, &config)
    );
    first.unwrap();
    second.unwrap();
    hypgm_ext_db::verify(&migration_url, &config).await.unwrap();
    let pool = hypgm_ext_postgres::create_pool(&hypgm_ext_postgres::PostgresConfig::new(
        db.users_url.clone(),
    ))
    .unwrap();
    exemplum_dmn_users::repo::check_ready(&pool).await.unwrap();
    let repo = PgUserRepo::new(pool.clone());
    let id = Uuid::new_v4();
    let created = repo.create(id, "Example User").await.unwrap();
    assert_eq!(created.id, id);
    assert_eq!(created.display_name, "Example User");
    assert_eq!(created.created_at, created.updated_at);
    assert_eq!(repo.get(id).await.unwrap(), created);
    let updated = repo.update_name(id, "Renamed User").await.unwrap();
    assert_eq!(updated.created_at, created.created_at);
    assert!(updated.updated_at >= created.updated_at);
    assert_eq!(repo.get(id).await.unwrap(), updated);
    for bad in ["", " ", " padded ", "bad\0name"] {
        assert!(matches!(
            repo.create(Uuid::new_v4(), bad).await,
            Err(RepoError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        repo.create(Uuid::new_v4(), &"界".repeat(201)).await,
        Err(RepoError::InvalidInput(_))
    ));
    let unicode = repo
        .create(Uuid::new_v4(), &"界".repeat(200))
        .await
        .unwrap();
    let mut seen = Vec::new();
    let mut page = PageRequest::new(None, 1).unwrap();
    loop {
        let result = repo.list(page).await.unwrap();
        seen.extend(result.users.iter().map(|user| user.id));
        match result.next_after {
            Some(cursor) => page = PageRequest::new(Some(cursor), 1).unwrap(),
            None => break,
        }
    }
    assert!(seen.contains(&id) && seen.contains(&unicode.id));
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
    repo.delete(id).await.unwrap();
    repo.delete(unicode.id).await.unwrap();
    assert!(matches!(repo.get(id).await, Err(RepoError::NotFound)));
    assert!(matches!(
        repo.update_name(id, "Missing").await,
        Err(RepoError::NotFound)
    ));
    assert!(matches!(repo.delete(id).await, Err(RepoError::NotFound)));
    for forbidden in [
        "CREATE TABLE exemplum.forbidden(id integer)",
        "SELECT * FROM infra._sqlx_migrations",
        "SELECT * FROM apalis._sqlx_migrations",
        "SELECT * FROM apalis.jobs",
        "SET ROLE exemplum_owner",
        "CREATE TEMP TABLE forbidden(id integer)",
    ] {
        assert!(
            sqlx::raw_sql(forbidden).execute(&pool).await.is_err(),
            "allowed {forbidden}"
        );
    }
    let tasks = sqlx::PgPool::connect(&db.tasks_url).await.unwrap();
    for forbidden in [
        "SELECT * FROM exemplum.users",
        "SELECT * FROM infra._sqlx_migrations",
        "SELECT * FROM apalis._sqlx_migrations",
        "CREATE TABLE apalis.forbidden(id integer)",
        "SET ROLE exemplum_owner",
    ] {
        assert!(
            sqlx::raw_sql(forbidden).execute(&tasks).await.is_err(),
            "allowed {forbidden}"
        );
    }
    // Readiness must reject the wrong role and an altered schema contract.
    assert!(exemplum_dmn_users::repo::check_ready(&tasks).await.is_err());
    let mut admin = PgConnection::connect(&db.admin_url).await.unwrap();
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint LANGUAGE sql AS 'SELECT 0::bigint'").execute(&mut admin).await.unwrap();
    assert!(exemplum_dmn_users::repo::check_ready(&pool).await.is_err());
    // Applied migrations do not silently rewrite objects after manual tampering.
    hypgm_ext_db::migrate(&migration_url, &config).await.unwrap();
    assert!(exemplum_dmn_users::repo::check_ready(&pool).await.is_err());
    assert!(hypgm_ext_db::verify(&migration_url, &config).await.is_err());
    pool.close().await;
    tasks.close().await;
}
