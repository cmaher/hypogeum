use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE SCHEMA IF NOT EXISTS fndapp AUTHORIZATION foundation_owner;\
                 REVOKE ALL ON SCHEMA fndapp FROM PUBLIC;",
            )
            .await?;

        let table = Table::create()
            .table((Alias::new("fndapp"), Alias::new("users")))
            .if_not_exists()
            .col(
                ColumnDef::new(Alias::new("id"))
                    .uuid()
                    .not_null()
                    .primary_key(),
            )
            .col(
                ColumnDef::new(Alias::new("display_name"))
                    .string()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("created_at"))
                    .timestamp_with_time_zone()
                    .not_null()
                    .default(Expr::cust("statement_timestamp()")),
            )
            .col(
                ColumnDef::new(Alias::new("updated_at"))
                    .timestamp_with_time_zone()
                    .not_null()
                    .default(Expr::cust("statement_timestamp()")),
            )
            .check(Expr::cust(
                "char_length(display_name) BETWEEN 1 AND 200 AND display_name !~ '^[[:space:]]|[[:space:]]$'",
            ))
            .to_owned();

        manager.create_table(table).await?;

        manager
            .get_connection()
            .execute_unprepared(
                "GRANT USAGE ON SCHEMA fndapp TO foundation_users;\
                 GRANT SELECT, INSERT, UPDATE, DELETE ON fndapp.users TO foundation_users;\
                 CREATE OR REPLACE FUNCTION fndapp.schema_version() RETURNS bigint \
                 LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path = pg_catalog, pg_temp \
                 AS $$ SELECT 1::bigint $$;\
                 REVOKE ALL ON FUNCTION fndapp.schema_version() FROM PUBLIC;\
                 GRANT EXECUTE ON FUNCTION fndapp.schema_version() TO foundation_users;",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP SCHEMA IF EXISTS fndapp CASCADE;")
            .await?;
        Ok(())
    }
}
