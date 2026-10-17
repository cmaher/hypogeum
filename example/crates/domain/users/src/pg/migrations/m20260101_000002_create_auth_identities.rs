use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let auth_identities = Table::create()
            .table((Alias::new("exemplum"), Alias::new("auth_identities")))
            .if_not_exists()
            .col(
                ColumnDef::new(Alias::new("id"))
                    .uuid()
                    .not_null()
                    .primary_key(),
            )
            .col(
                ColumnDef::new(Alias::new("user_id"))
                    .uuid()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("identity_type"))
                    .string()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("identifier"))
                    .string()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("secret_hash"))
                    .string()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("metadata"))
                    .json_binary()
                    .not_null()
                    .default(Expr::cust("'{}'::jsonb")),
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
            .col(
                ColumnDef::new(Alias::new("revoked_at"))
                    .timestamp_with_time_zone()
                    .null(),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_auth_identities_user_id")
                    .from(
                        (Alias::new("exemplum"), Alias::new("auth_identities")),
                        Alias::new("user_id"),
                    )
                    .to(
                        (Alias::new("exemplum"), Alias::new("users")),
                        Alias::new("id"),
                    )
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade),
            )
            .to_owned();

        manager.create_table(auth_identities).await?;

        let unique_ident_idx = Index::create()
            .name("idx_auth_identities_identity_unique")
            .table((Alias::new("exemplum"), Alias::new("auth_identities")))
            .col(Alias::new("identity_type"))
            .col(Alias::new("identifier"))
            .unique()
            .and_where(Expr::col(Alias::new("revoked_at")).is_null())
            .to_owned();

        manager.create_index(unique_ident_idx).await?;

        let user_id_idx = Index::create()
            .name("idx_auth_identities_user_id")
            .table((Alias::new("exemplum"), Alias::new("auth_identities")))
            .col(Alias::new("user_id"))
            .to_owned();

        manager.create_index(user_id_idx).await?;

        let sessions = Table::create()
            .table((Alias::new("exemplum"), Alias::new("sessions")))
            .if_not_exists()
            .col(
                ColumnDef::new(Alias::new("id"))
                    .string()
                    .not_null()
                    .primary_key(),
            )
            .col(
                ColumnDef::new(Alias::new("data"))
                    .binary()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("expiry_date"))
                    .timestamp_with_time_zone()
                    .not_null(),
            )
            .to_owned();

        manager.create_table(sessions).await?;

        let sessions_expiry_idx = Index::create()
            .name("idx_sessions_expiry_date")
            .table((Alias::new("exemplum"), Alias::new("sessions")))
            .col(Alias::new("expiry_date"))
            .to_owned();

        manager.create_index(sessions_expiry_idx).await?;

        manager
            .get_connection()
            .execute_unprepared(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON exemplum.auth_identities TO exemplum_users;\
                 GRANT SELECT, INSERT, UPDATE, DELETE ON exemplum.sessions TO exemplum_users;",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS exemplum.sessions CASCADE;\
                 DROP TABLE IF EXISTS exemplum.auth_identities CASCADE;",
            )
            .await?;
        Ok(())
    }
}
