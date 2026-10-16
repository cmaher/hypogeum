pub mod m20260101_000001_create_users;

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260101_000001_create_users::Migration)]
    }

    fn migration_table_name() -> DynIden {
        sea_orm_migration::prelude::SeaRc::new(sea_orm_migration::prelude::Alias::new(
            "seaql_migrations",
        ))
    }

    async fn install<C>(db: &C) -> Result<(), DbErr>
    where
        C: ConnectionTrait,
    {
        db.execute_unprepared(
            "CREATE SCHEMA IF NOT EXISTS fndapp AUTHORIZATION foundation_owner;\
             REVOKE ALL ON SCHEMA fndapp FROM PUBLIC;",
        )
        .await?;
        let builder = db.get_database_backend();
        let schema = sea_orm::Schema::new(builder);
        let mut stmt = schema.create_table_from_entity(sea_orm_migration::seaql_migrations::Entity);
        stmt.table(Self::migration_table_name());
        stmt.if_not_exists();
        db.execute(&stmt).await?;
        Ok(())
    }
}
