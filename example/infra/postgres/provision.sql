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
  FOREACH r IN ARRAY ARRAY['exemplum_owner','exemplum_migrator','exemplum_users','exemplum_tasks'] LOOP
    IF NOT EXISTS(SELECT FROM pg_catalog.pg_roles WHERE rolname=r) THEN
      EXECUTE format('CREATE ROLE %I',r);
    END IF;
    EXECUTE format('ALTER ROLE %I NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT',r);
  END LOOP;
END $$;
ALTER ROLE exemplum_owner NOLOGIN PASSWORD NULL;
ALTER ROLE exemplum_migrator LOGIN PASSWORD :'migration_password';
ALTER ROLE exemplum_users LOGIN PASSWORD :'users_password';
ALTER ROLE exemplum_tasks LOGIN PASSWORD :'tasks_password';
SELECT format('REVOKE %I FROM %I', parent.rolname, member.rolname)
  FROM pg_catalog.pg_auth_members m
  JOIN pg_catalog.pg_roles parent ON parent.oid=m.roleid
  JOIN pg_catalog.pg_roles member ON member.oid=m.member
  WHERE member.rolname IN ('exemplum_owner','exemplum_migrator','exemplum_users','exemplum_tasks') \gexec
GRANT exemplum_owner TO exemplum_migrator WITH INHERIT FALSE, SET TRUE;
COMMIT;
\ir database.sql
