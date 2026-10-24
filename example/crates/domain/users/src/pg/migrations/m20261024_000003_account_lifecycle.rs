use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE exemplum.users
               ADD COLUMN approval_status text NOT NULL DEFAULT 'approved'
                 CHECK (approval_status IN ('pending', 'approved')),
               ADD COLUMN is_admin boolean NOT NULL DEFAULT false,
               ADD COLUMN approved_at timestamptz,
               ADD COLUMN welcome_scheduled_at timestamptz,
               ADD COLUMN welcome_processed_at timestamptz;
             GRANT USAGE ON SCHEMA apalis TO exemplum_users;
             GRANT USAGE ON TYPE apalis.hstore TO exemplum_users;
             GRANT INSERT ON apalis.jobs TO exemplum_users;
             CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint
             LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp
             AS $$ SELECT 2::bigint $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "REVOKE INSERT ON apalis.jobs FROM exemplum_users;
             REVOKE USAGE ON TYPE apalis.hstore FROM exemplum_users;
             REVOKE USAGE ON SCHEMA apalis FROM exemplum_users;
             ALTER TABLE exemplum.users DROP COLUMN welcome_processed_at,
               DROP COLUMN welcome_scheduled_at, DROP COLUMN approved_at, DROP COLUMN is_admin, DROP COLUMN approval_status;
             CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint
             LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp
             AS $$ SELECT 1::bigint $$;",
            )
            .await?;
        Ok(())
    }
}
