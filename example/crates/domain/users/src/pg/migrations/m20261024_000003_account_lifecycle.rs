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
               ADD COLUMN approved_at timestamptz;
             -- Existing accounts were backfilled as approved above; any insert
             -- that omits the status must start pending, never approved.
             ALTER TABLE exemplum.users ALTER COLUMN approval_status SET DEFAULT 'pending';
             -- Account administration lists newest first, optionally by status.
             CREATE INDEX idx_users_created_at_id ON exemplum.users (created_at DESC, id DESC);
             CREATE INDEX idx_users_approval_status_created_at_id
               ON exemplum.users (approval_status, created_at DESC, id DESC);
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
                "DROP INDEX exemplum.idx_users_approval_status_created_at_id;
             DROP INDEX exemplum.idx_users_created_at_id;
             ALTER TABLE exemplum.users DROP COLUMN approved_at, DROP COLUMN is_admin,
               DROP COLUMN approval_status;
             CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint
             LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp
             AS $$ SELECT 1::bigint $$;",
            )
            .await?;
        Ok(())
    }
}
