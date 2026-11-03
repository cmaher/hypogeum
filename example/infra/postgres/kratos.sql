-- Ory Kratos gets its own login role and database. The role owns only that
-- database (Kratos migrates its own schema) and cannot connect elsewhere.
\getenv kratos_password KRATOS_PASSWORD
BEGIN;
SET LOCAL log_statement = 'none';
SET LOCAL log_min_duration_statement = -1;
SET LOCAL log_min_error_statement = 'panic';
SET LOCAL password_encryption = 'scram-sha-256';
DO $$ BEGIN
  IF NOT EXISTS(SELECT FROM pg_catalog.pg_roles WHERE rolname='kratos') THEN
    CREATE ROLE kratos;
  END IF;
END $$;
ALTER ROLE kratos NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT LOGIN PASSWORD :'kratos_password';
SELECT format('REVOKE %I FROM %I', parent.rolname, member.rolname)
  FROM pg_catalog.pg_auth_members m
  JOIN pg_catalog.pg_roles parent ON parent.oid=m.roleid
  JOIN pg_catalog.pg_roles member ON member.oid=m.member
  WHERE member.rolname='kratos' \gexec
COMMIT;
-- CREATE DATABASE cannot run inside a transaction block.
SELECT 'CREATE DATABASE kratos OWNER kratos'
  WHERE NOT EXISTS(SELECT FROM pg_catalog.pg_database WHERE datname='kratos') \gexec
ALTER DATABASE kratos OWNER TO kratos;
REVOKE ALL ON DATABASE kratos FROM PUBLIC;
