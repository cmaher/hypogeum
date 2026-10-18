use async_trait::async_trait;
use chrono::Utc;
use hypgm_errors::RepoError;
use sea_orm::entity::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    SqlxPostgresConnector,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::AuthRepo;
use crate::models::{ActiveModel, AuthIdentity, Column, Entity};

fn map_db_err(err: sea_orm::DbErr) -> RepoError {
    match err {
        sea_orm::DbErr::RecordNotFound(_) | sea_orm::DbErr::RecordNotUpdated => RepoError::NotFound,
        other => RepoError::database(other),
    }
}

#[derive(Clone)]
pub struct PgAuthRepo {
    db: DatabaseConnection,
}

impl PgAuthRepo {
    pub fn new(pool: PgPool) -> Self {
        Self {
            db: SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
        }
    }

    pub fn from_database(db: DatabaseConnection) -> Self {
        Self { db }
    }

    fn db(&self) -> &DatabaseConnection {
        &self.db
    }
}

#[async_trait]
impl AuthRepo for PgAuthRepo {
    async fn find_active_by_type_and_identifier(
        &self,
        identity_type: &str,
        identifier: &str,
    ) -> Result<Option<AuthIdentity>, RepoError> {
        let model = Entity::find()
            .filter(Column::IdentityType.eq(identity_type))
            .filter(Column::Identifier.eq(identifier))
            .filter(Column::RevokedAt.is_null())
            .one(self.db())
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn find_active_by_id(&self, id: Uuid) -> Result<Option<AuthIdentity>, RepoError> {
        let model = Entity::find_by_id(id)
            .filter(Column::RevokedAt.is_null())
            .one(self.db())
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<AuthIdentity>, RepoError> {
        let models = Entity::find()
            .filter(Column::UserId.eq(user_id))
            .order_by_asc(Column::CreatedAt)
            .all(self.db())
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn create(
        &self,
        id: Uuid,
        user_id: Uuid,
        identity_type: &str,
        identifier: &str,
        secret_hash: &str,
        metadata: serde_json::Value,
    ) -> Result<AuthIdentity, RepoError> {
        let active = ActiveModel {
            id: sea_orm::ActiveValue::Set(id),
            user_id: sea_orm::ActiveValue::Set(user_id),
            identity_type: sea_orm::ActiveValue::Set(identity_type.to_string()),
            identifier: sea_orm::ActiveValue::Set(identifier.to_string()),
            secret_hash: sea_orm::ActiveValue::Set(secret_hash.to_string()),
            metadata: sea_orm::ActiveValue::Set(metadata),
            created_at: sea_orm::ActiveValue::NotSet,
            updated_at: sea_orm::ActiveValue::NotSet,
            revoked_at: sea_orm::ActiveValue::NotSet,
        };
        let model = active.insert(self.db()).await.map_err(map_db_err)?;
        Ok(model.into())
    }

    async fn revoke(&self, id: Uuid) -> Result<(), RepoError> {
        let now = Utc::now();
        let res = Entity::update_many()
            .col_expr(Column::RevokedAt, sea_orm::sea_query::Expr::val(now))
            .col_expr(Column::UpdatedAt, sea_orm::sea_query::Expr::val(now))
            .filter(Column::Id.eq(id))
            .filter(Column::RevokedAt.is_null())
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
