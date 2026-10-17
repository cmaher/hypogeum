use async_trait::async_trait;
use hypgm_authn::{
    error::AuthError,
    principal::{AuthMethod, Principal},
    session::AuthBackend,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::repo::{AuthIdentity, AuthRepo, PgAuthRepo};

#[derive(Clone)]
pub struct ExemplumAuthBackend<R: AuthRepo = PgAuthRepo> {
    repo: Arc<R>,
}

impl<R: AuthRepo> ExemplumAuthBackend<R> {
    pub fn new(repo: R) -> Self {
        Self {
            repo: Arc::new(repo),
        }
    }

    pub fn from_arc(repo: Arc<R>) -> Self {
        Self { repo }
    }

    pub fn repo(&self) -> &Arc<R> {
        &self.repo
    }
}

#[async_trait]
impl<R: AuthRepo> AuthBackend for ExemplumAuthBackend<R> {
    async fn resolve_principal(&self, auth_id: Uuid) -> Result<Option<Principal>, AuthError> {
        let identity: Option<AuthIdentity> = self
            .repo
            .find_active_by_id(auth_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        match identity {
            Some(ident) => {
                if ident.revoked_at.is_some() {
                    Ok(None)
                } else {
                    Ok(Some(Principal::new(
                        ident.id,
                        ident.user_id,
                        AuthMethod::Session,
                    )))
                }
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use hypgm_errors::RepoError;
    use std::collections::HashMap;
    use tokio::sync::RwLock;

    #[derive(Default)]
    struct MockAuthRepo {
        identities: RwLock<HashMap<Uuid, AuthIdentity>>,
    }

    #[async_trait]
    impl AuthRepo for MockAuthRepo {
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

    #[tokio::test]
    async fn test_resolve_active_principal() {
        let repo = Arc::new(MockAuthRepo::default());
        let auth_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        repo.create(
            auth_id,
            user_id,
            "password",
            "testuser",
            "hash",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        let backend = ExemplumAuthBackend::from_arc(repo.clone());
        let principal = backend.resolve_principal(auth_id).await.unwrap().unwrap();

        assert_eq!(principal.auth_id, auth_id);
        assert_eq!(principal.user_id, user_id);
        assert_eq!(principal.method, AuthMethod::Session);
    }

    #[tokio::test]
    async fn test_resolve_revoked_principal_fails() {
        let repo = Arc::new(MockAuthRepo::default());
        let auth_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        repo.create(
            auth_id,
            user_id,
            "password",
            "testuser",
            "hash",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        repo.revoke(auth_id).await.unwrap();

        let backend = ExemplumAuthBackend::from_arc(repo);
        let principal = backend.resolve_principal(auth_id).await.unwrap();
        assert!(principal.is_none());
    }

    #[tokio::test]
    async fn test_resolve_nonexistent_principal() {
        let repo = Arc::new(MockAuthRepo::default());
        let backend = ExemplumAuthBackend::from_arc(repo);
        let principal = backend.resolve_principal(Uuid::new_v4()).await.unwrap();
        assert!(principal.is_none());
    }
}
