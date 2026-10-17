use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hypgm_errors::RepoError;
use sea_orm::entity::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    SqlxPostgresConnector,
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct InMemoryAuthRepo {
    identities: Arc<RwLock<HashMap<Uuid, AuthIdentity>>>,
}

impl InMemoryAuthRepo {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AuthRepo for InMemoryAuthRepo {
    async fn find_active_by_type_and_identifier(
        &self,
        identity_type: &str,
        identifier: &str,
    ) -> Result<Option<AuthIdentity>, RepoError> {
        let map = self.identities.read().await;
        for ident in map.values() {
            if ident.identity_type == identity_type
                && ident.identifier == identifier
                && ident.revoked_at.is_none()
            {
                return Ok(Some(ident.clone()));
            }
        }
        Ok(None)
    }

    async fn find_active_by_id(&self, id: Uuid) -> Result<Option<AuthIdentity>, RepoError> {
        let map = self.identities.read().await;
        if let Some(ident) = map.get(&id) {
            if ident.revoked_at.is_none() {
                return Ok(Some(ident.clone()));
            }
        }
        Ok(None)
    }

    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<AuthIdentity>, RepoError> {
        let map = self.identities.read().await;
        Ok(map
            .values()
            .filter(|i| i.user_id == user_id)
            .cloned()
            .collect())
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
        let now = Utc::now();
        let ident = AuthIdentity {
            id,
            user_id,
            identity_type: identity_type.to_string(),
            identifier: identifier.to_string(),
            secret_hash: secret_hash.to_string(),
            metadata,
            created_at: now,
            updated_at: now,
            revoked_at: None,
        };
        self.identities.write().await.insert(id, ident.clone());
        Ok(ident)
    }

    async fn revoke(&self, id: Uuid) -> Result<(), RepoError> {
        let mut map = self.identities.write().await;
        if let Some(ident) = map.get_mut(&id) {
            ident.revoked_at = Some(Utc::now());
            Ok(())
        } else {
            Err(RepoError::NotFound)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(schema_name = "exemplum", table_name = "auth_identities")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub identity_type: String,
    pub identifier: String,
    pub secret_hash: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthIdentity {
    pub id: Uuid,
    pub user_id: Uuid,
    pub identity_type: String,
    pub identifier: String,
    pub secret_hash: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<Model> for AuthIdentity {
    fn from(model: Model) -> Self {
        Self {
            id: model.id,
            user_id: model.user_id,
            identity_type: model.identity_type,
            identifier: model.identifier,
            secret_hash: model.secret_hash,
            metadata: model.metadata,
            created_at: model.created_at,
            updated_at: model.updated_at,
            revoked_at: model.revoked_at,
        }
    }
}

fn map_db_err(err: sea_orm::DbErr) -> RepoError {
    match err {
        sea_orm::DbErr::RecordNotFound(_) | sea_orm::DbErr::RecordNotUpdated => RepoError::NotFound,
        other => RepoError::database(other),
    }
}

#[async_trait]
pub trait AuthRepo: Send + Sync + 'static {
    async fn find_active_by_type_and_identifier(
        &self,
        identity_type: &str,
        identifier: &str,
    ) -> Result<Option<AuthIdentity>, RepoError>;

    async fn find_active_by_id(&self, id: Uuid) -> Result<Option<AuthIdentity>, RepoError>;

    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<AuthIdentity>, RepoError>;

    async fn create(
        &self,
        id: Uuid,
        user_id: Uuid,
        identity_type: &str,
        identifier: &str,
        secret_hash: &str,
        metadata: serde_json::Value,
    ) -> Result<AuthIdentity, RepoError>;

    async fn revoke(&self, id: Uuid) -> Result<(), RepoError>;
}

#[async_trait]
impl AuthRepo for Arc<dyn AuthRepo> {
    async fn find_active_by_type_and_identifier(
        &self,
        identity_type: &str,
        identifier: &str,
    ) -> Result<Option<AuthIdentity>, RepoError> {
        self.as_ref()
            .find_active_by_type_and_identifier(identity_type, identifier)
            .await
    }

    async fn find_active_by_id(&self, id: Uuid) -> Result<Option<AuthIdentity>, RepoError> {
        self.as_ref().find_active_by_id(id).await
    }

    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<AuthIdentity>, RepoError> {
        self.as_ref().find_by_user_id(user_id).await
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
        self.as_ref()
            .create(id, user_id, identity_type, identifier, secret_hash, metadata)
            .await
    }

    async fn revoke(&self, id: Uuid) -> Result<(), RepoError> {
        self.as_ref().revoke(id).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_identity_from_model() {
        let id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let now = Utc::now();
        let model = Model {
            id,
            user_id,
            identity_type: "password".to_string(),
            identifier: "alice".to_string(),
            secret_hash: "hash".to_string(),
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
            revoked_at: None,
        };
        let identity: AuthIdentity = model.into();
        assert_eq!(identity.id, id);
        assert_eq!(identity.user_id, user_id);
        assert_eq!(identity.identity_type, "password");
        assert_eq!(identity.identifier, "alice");
        assert_eq!(identity.secret_hash, "hash");
        assert!(identity.revoked_at.is_none());
    }
}

