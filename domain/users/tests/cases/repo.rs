use errors::RepoError;
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use test_support::TestDatabase;
use users::{
    models::PageRequest,
    repo::{PgUserRepo, UserRepo},
};
use uuid::Uuid;

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn existing_user_survives_migration_upgrade(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    use std::str::FromStr;
    let db = TestDatabase::provision(options).await;
    let owner = PgConnectOptions::from_str(&db.migration_url)
        .unwrap()
        .options([("role", "foundation_owner")]);
    let mut connection = PgConnection::connect_with(&owner).await.unwrap();
    // Reproduce an existing database with the unchanged first migration applied.
    let mut initial = sqlx::migrate!("../../crates/db/resources/migrations");
    initial.create_schema("infra");
    initial.dangerous_set_table_name("infra._sqlx_migrations");
    initial.run_to(1, &mut connection).await.unwrap();
    connection.close().await.unwrap();
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = PgUserRepo::new(pool.clone());
    let before = repo.create(Uuid::new_v4(), "Existing User").await.unwrap();
    assert!(users::repo::check_ready(&pool).await.is_err());
    db.migrate().await;
    assert_eq!(repo.get(before.id).await.unwrap(), before);
    users::repo::check_ready(&pool).await.unwrap();
    foundation_db::verify(&db.migration_url).await.unwrap();
    pool.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn user_repository_connects_and_persists_under_runtime_privileges(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    let db = TestDatabase::provision(options).await;
    let migration_url = db.migration_url.clone();
    // Concurrent runners must serialize and remain repeatable.
    let (first, second) = tokio::join!(
        foundation_db::migrate(&migration_url),
        foundation_db::migrate(&migration_url)
    );
    first.unwrap();
    second.unwrap();
    foundation_db::verify(&migration_url).await.unwrap();
    let pool = postgres::create_pool(&postgres::PostgresConfig::new(db.users_url.clone())).unwrap();
    users::repo::check_ready(&pool).await.unwrap();
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
        "CREATE TABLE app.forbidden(id integer)",
        "SELECT * FROM infra._sqlx_migrations",
        "SELECT * FROM apalis._sqlx_migrations",
        "SELECT * FROM apalis.jobs",
        "SET ROLE foundation_owner",
        "CREATE TEMP TABLE forbidden(id integer)",
    ] {
        assert!(
            sqlx::raw_sql(forbidden).execute(&pool).await.is_err(),
            "allowed {forbidden}"
        );
    }
    let tasks = sqlx::PgPool::connect(&db.tasks_url).await.unwrap();
    for forbidden in [
        "SELECT * FROM app.users",
        "SELECT * FROM infra._sqlx_migrations",
        "SELECT * FROM apalis._sqlx_migrations",
        "CREATE TABLE apalis.forbidden(id integer)",
        "SET ROLE foundation_owner",
    ] {
        assert!(
            sqlx::raw_sql(forbidden).execute(&tasks).await.is_err(),
            "allowed {forbidden}"
        );
    }
    // Readiness must reject the wrong role and an altered schema contract.
    assert!(users::repo::check_ready(&tasks).await.is_err());
    let mut admin = PgConnection::connect(&db.admin_url).await.unwrap();
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION app.schema_version() RETURNS bigint LANGUAGE sql AS 'SELECT 0::bigint'").execute(&mut admin).await.unwrap();
    assert!(users::repo::check_ready(&pool).await.is_err());
    // Applied migrations do not silently rewrite objects after manual tampering.
    foundation_db::migrate(&migration_url).await.unwrap();
    assert!(users::repo::check_ready(&pool).await.is_err());
    assert!(foundation_db::verify(&migration_url).await.is_err());
    pool.close().await;
    tasks.close().await;
}
