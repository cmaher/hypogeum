use exemplum_dmn_users::{
    models::PageRequest,
    repo::{PgUserRepo, UserRepo},
};
use hypgm_errors::RepoError;
use hypgm_test_support::TestDatabase;
use sea_orm_migration::MigratorTrait;
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use uuid::Uuid;

fn exemplum_config() -> hypgmx_rdbms::DatabaseConfig {
    hypgmx_rdbms::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
        .with_app_schema("exemplum")
        .with_domain_migrator(<dyn hypgmx_rdbms::DomainMigrator>::from_sea_orm::<
            exemplum_dmn_users::pg::migrations::Migrator,
        >())
        .with_service_contract(hypgmx_rdbms::ServiceContract::new(
            "exemplum",
            "exemplum_users",
            3,
            Some("m20261103_000004_identity_accounts"),
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
    let state: (String, bool, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT approval_status, is_admin, approved_at FROM exemplum.users WHERE id = $1",
    )
    .bind(before.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, ("approved".into(), false, None));
    // Only the backfill is approved; later inserts that omit the status start pending.
    let after = repo.create(Uuid::new_v4(), "New User").await.unwrap();
    let status: String =
        sqlx::query_scalar("SELECT approval_status FROM exemplum.users WHERE id = $1")
            .bind(after.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "pending");
    exemplum_dmn_users::repo::check_ready(&pool).await.unwrap();
    hypgmx_rdbms::verify(&db.exemplum_migration_url, &exemplum_config())
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
        hypgmx_rdbms::migrate(&migration_url, &config),
        hypgmx_rdbms::migrate(&migration_url, &config)
    );
    first.unwrap();
    second.unwrap();
    hypgmx_rdbms::verify(&migration_url, &config).await.unwrap();
    let pool =
        hypgmx_rdbms::create_pool(&hypgmx_rdbms::PoolConfig::new(db.users_url.clone())).unwrap();
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
        "UPDATE apalis.jobs SET status = 'Done'",
        "DELETE FROM apalis.jobs",
        "INSERT INTO apalis.jobs (id) VALUES ('forbidden')",
        "SELECT * FROM apalis.workers",
        "INSERT INTO apalis.workers (id) VALUES ('forbidden')",
        "CREATE TABLE apalis.forbidden(id integer)",
        "SET ROLE exemplum_owner",
        "CREATE TEMP TABLE forbidden(id integer)",
    ] {
        assert!(
            sqlx::raw_sql(forbidden).execute(&pool).await.is_err(),
            "allowed {forbidden}"
        );
    }
    let tasks = sqlx::PgPool::connect(&db.exemplum_tasks_url).await.unwrap();
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
    for dropped in ["exemplum.auth_identities", "exemplum.sessions"] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(dropped)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!exists, "{dropped} must be dropped");
    }
    // Readiness must reject the wrong role and an altered schema contract.
    assert!(exemplum_dmn_users::repo::check_ready(&tasks).await.is_err());
    let mut admin = PgConnection::connect(&db.admin_url).await.unwrap();
    exemplum_dmn_users::repo::check_ready(&pool).await.unwrap();
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint LANGUAGE sql AS 'SELECT 0::bigint'").execute(&mut admin).await.unwrap();
    assert!(exemplum_dmn_users::repo::check_ready(&pool).await.is_err());
    // Applied migrations do not silently rewrite objects after manual tampering.
    hypgmx_rdbms::migrate(&migration_url, &config)
        .await
        .unwrap();
    assert!(exemplum_dmn_users::repo::check_ready(&pool).await.is_err());
    assert!(hypgmx_rdbms::verify(&migration_url, &config).await.is_err());
    pool.close().await;
    tasks.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn identity_accounts_are_provisioned_once_and_paginated(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    use exemplum_dmn_users::accounts::{
        AccountError, AccountQuery, AccountRepo, AccountService, ApprovalStatus,
        DefaultAccountService,
    };
    use exemplum_dmn_users::repo::PgAccountRepo;
    use std::sync::Arc;
    let db = TestDatabase::provision(options).await;
    db.migrate_with(&exemplum_config()).await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = Arc::new(PgAccountRepo::new(pool.clone()));
    let service = DefaultAccountService::new(repo.clone());
    let first_id = Uuid::new_v4();
    let (a, b) = tokio::join!(
        service.ensure_account(first_id, "alice", "Alice"),
        service.ensure_account(first_id, "alice", "Alice")
    );
    let first = a.unwrap();
    assert_eq!(first, b.unwrap());
    assert_eq!(first.id, first_id);
    assert_eq!(first.display_name, "Alice");
    assert_eq!(first.approval_status, ApprovalStatus::Pending);
    assert!(!first.is_admin && first.approved_at.is_none());
    // Existing accounts are never changed by later provisioning.
    repo.approve(first_id).await.unwrap();
    let approved = service
        .ensure_account(first_id, "alice", "Renamed")
        .await
        .unwrap();
    assert_eq!(approved.display_name, "Alice");
    assert_eq!(approved.approval_status, ApprovalStatus::Approved);
    // Another identity cannot claim a stored username.
    assert!(matches!(
        service
            .ensure_account(Uuid::new_v4(), "alice", "Other")
            .await,
        Err(AccountError::Repo(_))
    ));
    for (username, name) in [("Alice", "Valid"), ("ab", "Valid"), ("valid", " padded ")] {
        assert!(matches!(
            service.ensure_account(Uuid::new_v4(), username, name).await,
            Err(AccountError::Validation)
        ));
    }
    let second = service
        .ensure_account(Uuid::new_v4(), "second", "Second")
        .await
        .unwrap();
    let third = service
        .ensure_account(Uuid::new_v4(), "third", "Third")
        .await
        .unwrap();
    // Pin registration times: `first` is newest; `second` and `third` tie, so
    // the id breaks the tie.
    let admin = sqlx::PgPool::connect(&db.admin_url).await.unwrap();
    for (id, at) in [
        (first.id, "2026-03-01T00:00:00Z"),
        (second.id, "2026-01-01T00:00:00Z"),
        (third.id, "2026-01-01T00:00:00Z"),
    ] {
        sqlx::query("UPDATE exemplum.users SET created_at = $2::timestamptz WHERE id = $1")
            .bind(id)
            .bind(at)
            .execute(&admin)
            .await
            .unwrap();
    }
    let (tie_high, tie_low) = if second.id > third.id {
        (second.id, third.id)
    } else {
        (third.id, second.id)
    };
    let list_all = |status| {
        let service = &service;
        async move {
            let mut seen = Vec::new();
            let mut cursor = None;
            loop {
                let page = service
                    .list(AccountQuery {
                        page: PageRequest::new(cursor, 1).unwrap(),
                        status,
                    })
                    .await
                    .unwrap();
                assert!(page.accounts.len() <= 1);
                for managed in page.accounts {
                    seen.push((managed.account.id, managed.username));
                }
                match page.next_after {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            seen
        }
    };
    let ids = |seen: Vec<(Uuid, Option<String>)>| -> Vec<Uuid> {
        seen.into_iter().map(|(id, _)| id).collect()
    };
    let all = list_all(None).await;
    assert!(all.contains(&(first.id, Some("alice".into()))));
    assert_eq!(ids(all), [first.id, tie_high, tie_low]);
    assert_eq!(
        ids(list_all(Some(ApprovalStatus::Pending)).await),
        [tie_high, tie_low]
    );
    assert_eq!(
        ids(list_all(Some(ApprovalStatus::Approved)).await),
        [first.id]
    );
    assert!(matches!(
        service
            .list(AccountQuery {
                page: PageRequest::new(Some(Uuid::new_v4()), 1).unwrap(),
                status: None,
            })
            .await,
        Err(AccountError::Repo(RepoError::InvalidInput(_)))
    ));
    pool.close().await;
    admin.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn approval_is_atomic_and_repeat_safe(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    use exemplum_dmn_users::{
        accounts::{AccountError, AccountRepo, ApprovalStatus},
        repo::PgAccountRepo,
    };
    let db = TestDatabase::provision(options).await;
    db.migrate_with(&exemplum_config()).await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = PgAccountRepo::new(pool.clone());
    let account = repo
        .ensure_account(Uuid::new_v4(), "approve", "Approve")
        .await
        .unwrap();
    let (a, b) = tokio::join!(repo.approve(account.id), repo.approve(account.id));
    let approved = a.unwrap();
    // The row lock serializes approvals, so both see the same first approval.
    assert_eq!(approved, b.unwrap());
    assert_eq!(approved.username.as_deref(), Some("approve"));
    let approved = approved.account;
    assert_eq!(approved.approval_status, ApprovalStatus::Approved);
    assert!(approved.approved_at.is_some());
    assert!(!approved.is_admin);
    assert_eq!(repo.approve(account.id).await.unwrap().account, approved);
    assert_eq!(repo.get(account.id).await.unwrap(), approved);
    assert!(matches!(
        repo.approve(Uuid::new_v4()).await,
        Err(AccountError::Repo(RepoError::NotFound))
    ));
    pool.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn promotion_approves_and_grants_admin_by_username(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    use exemplum_dmn_users::{
        accounts::{AccountError, AdminBootstrapService, ApprovalStatus, DefaultAccountService},
        repo::PgAccountRepo,
    };
    use std::sync::Arc;
    let db = TestDatabase::provision(options).await;
    db.migrate_with(&exemplum_config()).await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let repo = Arc::new(PgAccountRepo::new(pool.clone()));
    let service = DefaultAccountService::new(repo.clone());
    let pending = {
        use exemplum_dmn_users::accounts::AccountRepo;
        repo.ensure_account(Uuid::new_v4(), "pending-admin", "Pending")
            .await
            .unwrap()
    };
    let promoted = service.promote_admin("pending-admin").await.unwrap();
    assert_eq!(promoted.id, pending.id);
    assert_eq!(promoted.display_name, pending.display_name);
    assert!(promoted.is_admin);
    assert_eq!(promoted.approval_status, ApprovalStatus::Approved);
    assert!(promoted.approved_at.is_some());
    assert_eq!(
        service.promote_admin("pending-admin").await.unwrap(),
        promoted
    );
    assert!(matches!(
        service.promote_admin("missing").await,
        Err(AccountError::Repo(RepoError::NotFound))
    ));
    assert!(matches!(
        service.promote_admin(" Pending-Admin ").await,
        Err(AccountError::Validation)
    ));
    pool.close().await;
}
