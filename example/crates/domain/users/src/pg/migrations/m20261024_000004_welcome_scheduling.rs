use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(
            "ALTER TABLE exemplum.users ADD COLUMN welcome_scheduled_at timestamptz;
             UPDATE exemplum.users AS u
             SET welcome_scheduled_at = COALESCE(u.approved_at, u.welcome_processed_at, u.created_at)
             WHERE u.welcome_processed_at IS NOT NULL OR EXISTS (
                 SELECT 1 FROM apalis.jobs AS j
                 WHERE j.job_type = 'exemplum.welcome.v1'
                   AND convert_from(j.job, 'UTF8')::jsonb ->> 'user_id' = u.id::text
             );
             UPDATE apalis.jobs SET max_attempts = 2147483647
             WHERE job_type = 'exemplum.welcome.v1' AND max_attempts = 5
               AND attempts < max_attempts AND status IN ('Pending', 'Failed');
             CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint
             LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp
             AS $$ SELECT 3::bigint $$;"
        ).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE exemplum.users DROP COLUMN welcome_scheduled_at;
             CREATE OR REPLACE FUNCTION exemplum.schema_version() RETURNS bigint
             LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp
             AS $$ SELECT 2::bigint $$;",
            )
            .await?;
        Ok(())
    }
}
