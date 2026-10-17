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
        std::env::var_os("HYPOGEUM_TEST_CHILD").is_none(),
        "suite launcher must not run in child"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let init = PROVISION_SQL
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
            .replace("\\ir database.sql", DATABASE_SQL);
        let container = Postgres::default()
            .with_user("hypogeum_admin")
            .with_password("test-admin")
            .with_db_name("hypogeum_test")
            .with_init_sql(init.into_bytes())
            .with_tag("18.3-bookworm")
            .start()
            .await
            .expect("Start PostgreSQL testcontainer (Docker must be running)");
        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let url = format!("postgres://hypogeum_admin:test-admin@{host}:{port}/hypogeum_test");
        // Child environment is fixed before its test threads start; never set_var.
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--ignored", "--nocapture"])
            .env("DATABASE_URL", url)
            .env("HYPOGEUM_TEST_CHILD", "1");
        let output = child.output().await.expect("Launch SQLx integration cases");
        container
            .rm()
            .await
            .expect("Remove PostgreSQL testcontainer");
        assert!(
            output.status.success(),
            "SQLx integration suite failed with status {}\nSTDOUT:\n{}\nSTDERR:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    });
}

pub struct TestDatabase {
    pub migration_url: String,
    pub users_url: String,
    pub tasks_url: String,
    pub admin_url: String,
    pub exemplum_migration_url: String,
    pub exemplum_users_url: String,
    pub exemplum_tasks_url: String,
}
impl TestDatabase {
    /// SQLx has already created the unique test database; apply only its local
    /// grants/defaults here. Cluster roles were provisioned once at startup.
    pub async fn provision(options: PgConnectOptions) -> Self {
        let admin = PgPool::connect_with(options.clone()).await.unwrap();
        sqlx::raw_sql(DATABASE_SQL).execute(&admin).await.unwrap();
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
            migration_url: url("hypogeum_migrator", "test-migration"),
            users_url: url("exemplum_users", "test-users"),
            tasks_url: url("hypogeum_tasks", "test-tasks"),
            admin_url: options.to_url_lossy().to_string(),
            exemplum_migration_url: url("exemplum_migrator", "test-migration"),
            exemplum_users_url: url("exemplum_users", "test-users"),
            exemplum_tasks_url: url("exemplum_tasks", "test-tasks"),
        }
    }
    pub async fn migrate(&self) {
        self.migrate_with(&test_config()).await;
    }

    pub async fn migrate_with(&self, config: &hypgmx_rdbms::DatabaseConfig) {
        let migration_url = if config.migrator_role == "exemplum_migrator" {
            &self.exemplum_migration_url
        } else {
            &self.migration_url
        };
        hypgmx_rdbms::migrate(migration_url, config).await.unwrap();
    }
}

pub fn test_config() -> hypgmx_rdbms::DatabaseConfig {
    hypgmx_rdbms::DatabaseConfig {
        migrator_role: "hypogeum_migrator".into(),
        owner_role: "hypogeum_owner".into(),
        tasks_role: "hypogeum_tasks".into(),
        app_schemas: vec![],
        domain_migrators: vec![],
        service_contracts: vec![],
    }
}

const PROVISION_SQL: &str = r#"\set ON_ERROR_STOP on
\set ECHO none
\getenv migration_password MIGRATION_PASSWORD
\getenv users_password USERS_PASSWORD
\getenv tasks_password TASKS_PASSWORD
BEGIN;
SET LOCAL log_statement = 'none';
SET LOCAL log_min_duration_statement = -1;
SET LOCAL log_min_error_statement = 'panic';
SET LOCAL password_encryption = 'scram-sha-256';
DO $$ DECLARE r text; BEGIN
  FOREACH r IN ARRAY ARRAY['hypogeum_owner','hypogeum_migrator','hypogeum_users','hypogeum_tasks','exemplum_owner','exemplum_migrator','exemplum_users','exemplum_tasks'] LOOP
    IF NOT EXISTS(SELECT FROM pg_catalog.pg_roles WHERE rolname=r) THEN
      EXECUTE format('CREATE ROLE %I',r);
    END IF;
    EXECUTE format('ALTER ROLE %I NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT',r);
  END LOOP;
END $$;
ALTER ROLE hypogeum_owner NOLOGIN PASSWORD NULL;
ALTER ROLE hypogeum_migrator LOGIN PASSWORD :'migration_password';
ALTER ROLE hypogeum_users LOGIN PASSWORD :'users_password';
ALTER ROLE hypogeum_tasks LOGIN PASSWORD :'tasks_password';
ALTER ROLE exemplum_owner NOLOGIN PASSWORD NULL;
ALTER ROLE exemplum_migrator LOGIN PASSWORD :'migration_password';
ALTER ROLE exemplum_users LOGIN PASSWORD :'users_password';
ALTER ROLE exemplum_tasks LOGIN PASSWORD :'tasks_password';
SELECT format('REVOKE %I FROM %I', parent.rolname, member.rolname)
  FROM pg_catalog.pg_auth_members m
  JOIN pg_catalog.pg_roles parent ON parent.oid=m.roleid
  JOIN pg_catalog.pg_roles member ON member.oid=m.member
  WHERE member.rolname IN ('hypogeum_owner','hypogeum_migrator','hypogeum_users','hypogeum_tasks','exemplum_owner','exemplum_migrator','exemplum_users','exemplum_tasks') \gexec
GRANT hypogeum_owner TO hypogeum_migrator WITH INHERIT FALSE, SET TRUE;
GRANT exemplum_owner TO exemplum_migrator WITH INHERIT FALSE, SET TRUE;
COMMIT;
\ir database.sql
"#;

const DATABASE_SQL: &str = r#"BEGIN;
DO $$ DECLARE db text := current_database(); BEGIN
  EXECUTE format('ALTER DATABASE %I OWNER TO hypogeum_owner',db);
  EXECUTE format('REVOKE ALL ON DATABASE %I FROM PUBLIC',db);
  EXECUTE format('GRANT CONNECT ON DATABASE %I TO hypogeum_migrator,hypogeum_users,hypogeum_tasks,exemplum_owner,exemplum_migrator,exemplum_users,exemplum_tasks',db);
  EXECUTE format('GRANT CREATE, TEMPORARY ON DATABASE %I TO exemplum_owner',db);
  EXECUTE format('ALTER ROLE hypogeum_users IN DATABASE %I SET search_path=pg_catalog',db);
  EXECUTE format('ALTER ROLE hypogeum_migrator IN DATABASE %I SET search_path=pg_catalog',db);
  EXECUTE format('ALTER ROLE hypogeum_tasks IN DATABASE %I SET search_path=pg_catalog,apalis',db);
  EXECUTE format('ALTER ROLE exemplum_users IN DATABASE %I SET search_path=pg_catalog',db);
  EXECUTE format('ALTER ROLE exemplum_migrator IN DATABASE %I SET search_path=pg_catalog',db);
  EXECUTE format('ALTER ROLE exemplum_tasks IN DATABASE %I SET search_path=pg_catalog,apalis',db);
END $$;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT ALL ON SCHEMA public TO hypogeum_owner, exemplum_owner;
ALTER DEFAULT PRIVILEGES FOR ROLE hypogeum_owner REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE hypogeum_owner REVOKE USAGE ON TYPES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE exemplum_owner REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE exemplum_owner REVOKE USAGE ON TYPES FROM PUBLIC;
DO $$ DECLARE bootstrap text; BEGIN
  SELECT rolname INTO STRICT bootstrap FROM pg_catalog.pg_roles WHERE oid=10;
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC',bootstrap);
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE USAGE ON TYPES FROM PUBLIC',bootstrap);
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I GRANT EXECUTE ON FUNCTIONS TO hypogeum_owner,exemplum_owner WITH GRANT OPTION',bootstrap);
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I GRANT USAGE ON TYPES TO hypogeum_owner,exemplum_owner WITH GRANT OPTION',bootstrap);
END $$;
COMMIT;
"#;
