pub mod postgres;

use async_trait::async_trait;
use chrono::Utc;
use hypgm_errors::RepoError;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use crate::models::AuthIdentity;
pub use postgres::PgAuthRepo;

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
        let map = self.identities.read().unwrap();
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
        let map = self.identities.read().unwrap();
        Ok(map.get(&id).filter(|i| i.revoked_at.is_none()).cloned())
    }

    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<AuthIdentity>, RepoError> {
        let map = self.identities.read().unwrap();
        let list: Vec<AuthIdentity> = map
            .values()
            .filter(|i| i.user_id == user_id)
            .cloned()
            .collect();
        Ok(list)
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
        let mut map = self.identities.write().unwrap();
        map.insert(id, ident.clone());
        Ok(ident)
    }

    async fn revoke(&self, id: Uuid) -> Result<(), RepoError> {
        let mut map = self.identities.write().unwrap();
        if let Some(ident) = map.get_mut(&id) {
            ident.revoked_at = Some(Utc::now());
            Ok(())
        } else {
            Err(RepoError::NotFound)
        }
    }
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
            .create(
                id,
                user_id,
                identity_type,
                identifier,
                secret_hash,
                metadata,
            )
            .await
    }

    async fn revoke(&self, id: Uuid) -> Result<(), RepoError> {
        self.as_ref().revoke(id).await
    }
}
