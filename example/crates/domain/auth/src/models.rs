use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
