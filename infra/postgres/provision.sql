\set ON_ERROR_STOP on
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
  FOREACH r IN ARRAY ARRAY['foundation_owner','foundation_migrator','foundation_users','foundation_tasks'] LOOP
    IF NOT EXISTS(SELECT FROM pg_catalog.pg_roles WHERE rolname=r) THEN
      EXECUTE format('CREATE ROLE %I',r);
    END IF;
    EXECUTE format('ALTER ROLE %I NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT',r);
  END LOOP;
END $$;
ALTER ROLE foundation_owner NOLOGIN PASSWORD NULL;
ALTER ROLE foundation_migrator LOGIN PASSWORD :'migration_password';
ALTER ROLE foundation_users LOGIN PASSWORD :'users_password';
ALTER ROLE foundation_tasks LOGIN PASSWORD :'tasks_password';
SELECT format('REVOKE %I FROM %I', parent.rolname, member.rolname)
  FROM pg_catalog.pg_auth_members m
  JOIN pg_catalog.pg_roles parent ON parent.oid=m.roleid
  JOIN pg_catalog.pg_roles member ON member.oid=m.member
  WHERE member.rolname IN ('foundation_owner','foundation_migrator','foundation_users','foundation_tasks') \gexec
GRANT foundation_owner TO foundation_migrator WITH INHERIT FALSE, SET TRUE;
SELECT format('ALTER DATABASE %I OWNER TO foundation_owner',current_database()) \gexec
SELECT format('REVOKE ALL ON DATABASE %I FROM PUBLIC',current_database()) \gexec
SELECT format('GRANT CONNECT ON DATABASE %I TO foundation_migrator,foundation_users,foundation_tasks',current_database()) \gexec
REVOKE ALL ON SCHEMA public FROM PUBLIC;
SELECT format('ALTER ROLE %I IN DATABASE %I SET search_path=pg_catalog',r,current_database())
  FROM unnest(ARRAY['foundation_users','foundation_migrator']) AS r \gexec
SELECT format('ALTER ROLE foundation_tasks IN DATABASE %I SET search_path=pg_catalog,apalis',current_database()) \gexec
SELECT format('REVOKE CONNECT ON DATABASE %I FROM PUBLIC,foundation_users,foundation_tasks',datname)
  FROM pg_catalog.pg_database WHERE datname='postgres' AND datname<>current_database() \gexec
-- Trusted extensions create contained objects as the bootstrap superuser.
-- Give only the owner the ability to delegate their runtime privileges.
DO $$ DECLARE bootstrap text; BEGIN
  SELECT rolname INTO STRICT bootstrap FROM pg_catalog.pg_roles WHERE oid=10;
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC',bootstrap);
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE USAGE ON TYPES FROM PUBLIC',bootstrap);
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I GRANT EXECUTE ON FUNCTIONS TO foundation_owner WITH GRANT OPTION',bootstrap);
  EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I GRANT USAGE ON TYPES TO foundation_owner WITH GRANT OPTION',bootstrap);
END $$;
COMMIT;
