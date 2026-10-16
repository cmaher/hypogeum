CREATE SCHEMA app AUTHORIZATION foundation_owner;
REVOKE ALL ON SCHEMA app FROM PUBLIC;
CREATE TABLE app.users (
    id uuid PRIMARY KEY,
    display_name text NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 200 AND display_name !~ '^[[:space:]]|[[:space:]]$'),
    created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT statement_timestamp()
);
GRANT USAGE ON SCHEMA app TO foundation_users;
GRANT SELECT, INSERT, UPDATE, DELETE ON app.users TO foundation_users;
