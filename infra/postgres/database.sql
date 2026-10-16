BEGIN;
DO $$ DECLARE db text := current_database(); BEGIN
  EXECUTE format('ALTER DATABASE %I OWNER TO foundation_owner',db);
  EXECUTE format('REVOKE ALL ON DATABASE %I FROM PUBLIC',db);
  EXECUTE format('GRANT CONNECT ON DATABASE %I TO foundation_migrator,foundation_users,foundation_tasks',db);
  EXECUTE format('ALTER ROLE foundation_users IN DATABASE %I SET search_path=pg_catalog',db);
  EXECUTE format('ALTER ROLE foundation_migrator IN DATABASE %I SET search_path=pg_catalog',db);
  EXECUTE format('ALTER ROLE foundation_tasks IN DATABASE %I SET search_path=pg_catalog,apalis',db);
END $$;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE foundation_owner REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE foundation_owner REVOKE USAGE ON TYPES FROM PUBLIC;
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
