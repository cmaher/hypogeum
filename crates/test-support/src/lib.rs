//! Test-only suite bootstrap. One container per suite, SQLx-owned database per test.
use sqlx::{ConnectOptions, PgPool, postgres::PgConnectOptions};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ImageExt, runners::AsyncRunner},
};

/// Run this from an ordinary #[test]; mark SQLx cases ignored so only the child
/// harness executes them. Child failures fail the enclosing suite.
pub fn run_suite() {
    assert!(
        std::env::var_os("FOUNDATION_TEST_CHILD").is_none(),
        "suite launcher must not run in child"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let init = include_str!("../../../infra/postgres/provision.sql")
            .replace(
                "\\getenv migration_password MIGRATION_PASSWORD",
                "\\set migration_password 'test-migration'",
            )
            .replace(
                "\\getenv users_password USERS_PASSWORD",
                "\\set users_password 'test-users'",
            )
            .replace(
                "\\getenv tasks_password TASKS_PASSWORD",
                "\\set tasks_password 'test-tasks'",
            )
            .replace(
                "\\ir database.sql",
                include_str!("../../../infra/postgres/database.sql"),
            );
        let container = Postgres::default()
            .with_user("foundation_admin")
            .with_password("test-admin")
            .with_db_name("foundation_test")
            .with_init_sql(init.into_bytes())
            .with_tag("18.3-bookworm")
            .start()
            .await
            .expect("Start PostgreSQL testcontainer (Docker must be running)");
        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let url = format!("postgres://foundation_admin:test-admin@{host}:{port}/foundation_test");
        // Child environment is fixed before its test threads start; never set_var.
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--ignored", "--nocapture"])
            .env("DATABASE_URL", url)
            .env("FOUNDATION_TEST_CHILD", "1");
        let status = child.status().await.expect("Launch SQLx integration cases");
        container
            .rm()
            .await
            .expect("Remove PostgreSQL testcontainer");
        assert!(status.success(), "SQLx integration suite failed: {status}");
    });
}

pub struct TestDatabase {
    pub migration_url: String,
    pub users_url: String,
    pub tasks_url: String,
    pub admin_url: String,
}
impl TestDatabase {
    /// SQLx has already created the unique test database; apply only its local
    /// grants/defaults here. Cluster roles were provisioned once at startup.
    pub async fn provision(options: PgConnectOptions) -> Self {
        let admin = PgPool::connect_with(options.clone()).await.unwrap();
        sqlx::raw_sql(include_str!("../../../infra/postgres/database.sql"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let url = |user, password| {
            options
                .clone()
                .username(user)
                .password(password)
                .to_url_lossy()
                .to_string()
        };
        Self {
            migration_url: url("foundation_migrator", "test-migration"),
            users_url: url("foundation_users", "test-users"),
            tasks_url: url("foundation_tasks", "test-tasks"),
            admin_url: options.to_url_lossy().to_string(),
        }
    }
    pub async fn migrate(&self) {
        foundation_db::migrate(&self.migration_url).await.unwrap();
    }
}
