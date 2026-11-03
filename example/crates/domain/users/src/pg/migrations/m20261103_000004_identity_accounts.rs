use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Credentials and sessions moved to the identity service; their tables
        // and data are dropped. Accounts are keyed by identity id from here on.
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE exemplum.sessions;
             DROP TABLE exemplum.auth_identities;
             ALTER TABLE exemplum.users ADD COLUMN username text
               CONSTRAINT users_username_key UNIQUE
               CHECK (username ~ '^[a-z0-9][a-z0-9_.-]{2,63}$');
             CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint
             LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp
             AS $$ SELECT 3::bigint $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Dropped credential and session tables are not restored.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE exemplum.users DROP COLUMN username;
             CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint
             LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp
             AS $$ SELECT 2::bigint $$;",
            )
            .await?;
        Ok(())
    }
}
