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
            2,
            Some("m20261024_000003_account_lifecycle"),
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
        "SELECT * FROM exemplum.auth_identities",
        "SELECT * FROM exemplum.sessions",
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
    assert!(
        sqlx::raw_sql("SELECT count(*) FROM exemplum.auth_identities")
            .execute(&pool)
            .await
            .is_ok()
    );
    assert!(
        sqlx::raw_sql("SELECT count(*) FROM exemplum.sessions")
            .execute(&pool)
            .await
            .is_ok()
    );
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
async fn auth_identities_and_sessions_schema_and_constraints_enforced(
    _pool_options: sqlx::postgres::PgPoolOptions,
    options: PgConnectOptions,
) {
    let db = TestDatabase::provision(options).await;
    db.migrate_with(&exemplum_config()).await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();

    let user_id = Uuid::new_v4();
    let repo = PgUserRepo::new(pool.clone());
    repo.create(user_id, "Auth Test User").await.unwrap();

    let auth_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exemplum.auth_identities (id, user_id, identity_type, identifier, secret_hash) \
         VALUES ($1, $2, 'password', 'alice', 'hash1')",
    )
    .bind(auth_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    // Duplicate active identifier must fail unique constraint
    let dup_res = sqlx::query(
        "INSERT INTO exemplum.auth_identities (id, user_id, identity_type, identifier, secret_hash) \
         VALUES ($1, $2, 'password', 'alice', 'hash2')",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .execute(&pool)
    .await;
    assert!(dup_res.is_err());

    // Revoking allows same identifier to be registered again
    sqlx::query(
        "UPDATE exemplum.auth_identities SET revoked_at = statement_timestamp() WHERE id = $1",
    )
    .bind(auth_id)
    .execute(&pool)
    .await
    .unwrap();

    let new_auth_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exemplum.auth_identities (id, user_id, identity_type, identifier, secret_hash) \
         VALUES ($1, $2, 'password', 'alice', 'hash2')",
    )
    .bind(new_auth_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    // Foreign key constraint on non-existent user
    let bad_fk = sqlx::query(
        "INSERT INTO exemplum.auth_identities (id, user_id, identity_type, identifier, secret_hash) \
         VALUES ($1, $2, 'password', 'bob', 'hash3')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await;
    assert!(bad_fk.is_err());

    // Deleting user cascades to auth_identities
    repo.delete(user_id).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM exemplum.auth_identities WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);

    // Sessions table operations
    sqlx::query(
        "INSERT INTO exemplum.sessions (id, data, expiry_date) VALUES ('sess1', $1, statement_timestamp() + interval '1 hour')",
    )
    .bind(b"session-data" as &[u8])
    .execute(&pool)
    .await
    .unwrap();

    pool.close().await;
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn account_registration_is_atomic_and_paginated(
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
    let first = service
        .register(" Alice ", " Alice ", "validated-hash")
        .await
        .unwrap();
    assert_eq!(first.display_name, "Alice");
    assert_eq!(first.approval_status, ApprovalStatus::Pending);
    assert!(!first.is_admin);
    assert!(first.approved_at.is_none());
    assert_eq!(repo.get(first.id).await.unwrap(), first);
    assert!(matches!(
        service.register("Duplicate", "Alice", "hash").await,
        Err(AccountError::DuplicateUsername)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM exemplum.users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "failed identity insert must roll back user");
    // An unrelated database failure must also roll back the user row.
    let admin = sqlx::PgPool::connect(&db.admin_url).await.unwrap();
    sqlx::query("ALTER TABLE exemplum.auth_identities ADD CONSTRAINT reject_test_identity CHECK (identifier <> 'blocked')").execute(&admin).await.unwrap();
    assert!(matches!(
        service.register("Rejected", "blocked", "hash").await,
        Err(AccountError::Repo(_))
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM exemplum.users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let second = service.register("Other", "alice", "hash").await.unwrap();
    let third = service.register("Third", "third", "hash").await.unwrap();
    // Pin registration times: `first` is newest; `second` and `third` tie, so
    // the id breaks the tie.
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
    repo.approve(third.id).await.unwrap();
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
                    let json = serde_json::to_string(&managed).unwrap();
                    assert!(!json.contains("hash"));
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
    assert!(all.contains(&(first.id, Some("Alice".into()))));
    assert!(all.contains(&(second.id, Some("alice".into()))));
    assert_eq!(ids(all), [first.id, tie_high, tie_low]);
    assert_eq!(
        ids(list_all(Some(ApprovalStatus::Pending)).await),
        [first.id, second.id]
    );
    assert_eq!(
        ids(list_all(Some(ApprovalStatus::Approved)).await),
        [third.id]
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
    for (name, username, hash) in [
        ("", "valid", "hash"),
        ("Valid", "bad\0name", "hash"),
        ("Valid", "valid", ""),
    ] {
        assert!(matches!(
            service.register(name, username, hash).await,
            Err(AccountError::Validation)
        ));
    }
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
    let account = repo.register("Approve", "approve", "hash").await.unwrap();
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
async fn administrator_creation_rejects_existing_usernames_unless_promoted(
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
    let fresh = repo
        .create_admin("Administrator", "admin", "original-hash")
        .await
        .unwrap();
    assert!(fresh.is_admin);
    assert_eq!(fresh.approval_status, ApprovalStatus::Approved);
    assert!(fresh.approved_at.is_some());
    assert!(matches!(
        repo.create_admin("Changed", "admin", "replacement").await,
        Err(AccountError::DuplicateUsername)
    ));
    assert_eq!(repo.promote_admin("admin").await.unwrap(), fresh);

    // An existing registration is never silently promoted by creation.
    let pending = repo
        .register("Pending", "pending-admin", "pending-hash")
        .await
        .unwrap();
    assert!(matches!(
        repo.create_admin("Ignored", "pending-admin", "replacement")
            .await,
        Err(AccountError::DuplicateUsername)
    ));
    let unchanged = repo.get(pending.id).await.unwrap();
    assert!(!unchanged.is_admin);
    assert_eq!(unchanged.approval_status, ApprovalStatus::Pending);

    let promoted = repo.promote_admin(" pending-admin ").await.unwrap();
    assert_eq!(promoted.id, pending.id);
    assert_eq!(promoted.display_name, pending.display_name);
    assert!(promoted.is_admin);
    assert_eq!(promoted.approval_status, ApprovalStatus::Approved);
    assert!(matches!(
        repo.promote_admin("missing").await,
        Err(AccountError::Repo(hypgm_errors::RepoError::NotFound))
    ));
    let hashes: Vec<String> =
        sqlx::query_scalar("SELECT secret_hash FROM exemplum.auth_identities ORDER BY identifier")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(hashes, ["original-hash", "pending-hash"]);
    let admin = sqlx::PgPool::connect(&db.admin_url).await.unwrap();
    sqlx::query("ALTER TABLE exemplum.auth_identities ADD CONSTRAINT reject_bootstrap CHECK (identifier <> 'blocked')").execute(&admin).await.unwrap();
    assert!(
        repo.create_admin("Rejected", "blocked", "hash")
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM exemplum.users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}
