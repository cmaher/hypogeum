use super::UserRepo;
use super::entity::{ActiveModel, Column, Entity};
use crate::models::{PageRequest, User, UserPage, validate_name};
use async_trait::async_trait;
use errors::RepoError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, SqlxPostgresConnector,
};
use sqlx::PgPool;
use uuid::Uuid;

pub struct PgUserRepo {
    db: DatabaseConnection,
}

impl PgUserRepo {
    pub fn new(pool: PgPool) -> Self {
        Self {
            db: SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
        }
    }

    fn db(&self) -> &DatabaseConnection {
        &self.db
    }
}

fn map_db_err(err: sea_orm::DbErr) -> RepoError {
    match err {
        sea_orm::DbErr::RecordNotFound(_) | sea_orm::DbErr::RecordNotUpdated => RepoError::NotFound,
        other => RepoError::database(other),
    }
}

#[async_trait]
impl UserRepo for PgUserRepo {
    async fn create(&self, id: Uuid, name: &str) -> Result<User, RepoError> {
        validate_name(name)?;
        let active = ActiveModel {
            id: sea_orm::ActiveValue::Set(id),
            display_name: sea_orm::ActiveValue::Set(name.to_string()),
            created_at: sea_orm::ActiveValue::NotSet,
            updated_at: sea_orm::ActiveValue::NotSet,
        };
        let model = active.insert(self.db()).await.map_err(map_db_err)?;
        Ok(model.into())
    }

    async fn get(&self, id: Uuid) -> Result<User, RepoError> {
        let model = Entity::find_by_id(id)
            .one(self.db())
            .await
            .map_err(map_db_err)?
            .ok_or(RepoError::NotFound)?;
        Ok(model.into())
    }

    async fn list(&self, page: PageRequest) -> Result<UserPage, RepoError> {
        let mut select = Entity::find()
            .order_by_asc(Column::Id)
            .limit(u64::from(page.limit()) + 1);
        if let Some(after) = page.after() {
            select = select.filter(Column::Id.gt(after));
        }
        let models = select.all(self.db()).await.map_err(map_db_err)?;
        let mut users: Vec<User> = models.into_iter().map(User::from).collect();
        let more = users.len() > page.limit() as usize;
        users.truncate(page.limit() as usize);
        let next_after = if more {
            users.last().map(|user| user.id)
        } else {
            None
        };
        Ok(UserPage { users, next_after })
    }

    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, RepoError> {
        validate_name(name)?;
        let active = ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(id),
            display_name: sea_orm::ActiveValue::Set(name.to_string()),
            updated_at: sea_orm::ActiveValue::Set(chrono::Utc::now()),
            created_at: sea_orm::ActiveValue::NotSet,
        };
        let model = active.update(self.db()).await.map_err(map_db_err)?;
        Ok(model.into())
    }

    async fn delete(&self, id: Uuid) -> Result<(), RepoError> {
        let res = Entity::delete_by_id(id)
            .exec(self.db())
            .await
            .map_err(map_db_err)?;
        if res.rows_affected == 0 {
            Err(RepoError::NotFound)
        } else {
            Ok(())
        }
    }
}

pub async fn check_ready(pool: &PgPool) -> Result<(), RepoError> {
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(pool)
        .await
        .map_err(RepoError::database)?;
    if role != "hypogeum_users" {
        return Err(RepoError::InvalidInput(
            "Use hypogeum_users runtime credentials".into(),
        ));
    }
    let version: i64 = sqlx::query_scalar("SELECT hypogeum.schema_version()")
        .fetch_one(pool)
        .await
        .map_err(RepoError::database)?;
    if version != 1 {
        return Err(RepoError::InvalidInput(
            "Users schema is not ready; run explicit migrations".into(),
        ));
    }
    Ok(())
}
