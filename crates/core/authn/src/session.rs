use crate::principal::Principal;
use async_trait::async_trait;
use axum_login::AuthUser;
use std::convert::Infallible;
use uuid::Uuid;

impl AuthUser for Principal {
    type Id = Uuid;

    fn id(&self) -> Self::Id {
        self.auth_id
    }

    fn session_auth_hash(&self) -> &[u8] {
        self.auth_id.as_bytes()
    }
}

/// Generic trait allowing consuming applications to resolve authentication identities into live Principals.
#[async_trait]
pub trait AuthBackend: Send + Sync + 'static {
    async fn resolve_principal(&self, auth_id: Uuid) -> Option<Principal>;
}

/// Adapter wrapping an [`AuthBackend`] to implement [`axum_login::AuthnBackend`].
#[derive(Debug, Clone, Default)]
pub struct PrincipalAuthBackend<B> {
    backend: B,
}

impl<B> PrincipalAuthBackend<B> {
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn inner(&self) -> &B {
        &self.backend
    }
}

impl<B: AuthBackend + Clone> axum_login::AuthnBackend for PrincipalAuthBackend<B> {
    type User = Principal;
    type Credentials = ();
    type Error = Infallible;

    async fn authenticate(
        &self,
        _creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        Ok(None)
    }

    async fn get_user(
        &self,
        user_id: &axum_login::UserId<Self>,
    ) -> Result<Option<Self::User>, Self::Error> {
        Ok(self.backend.resolve_principal(*user_id).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::principal::AuthMethod;
    use axum_login::AuthnBackend;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[derive(Clone, Default)]
    struct MockAuthBackend {
        principals: Arc<RwLock<HashMap<Uuid, Principal>>>,
    }

    #[async_trait]
    impl AuthBackend for MockAuthBackend {
        async fn resolve_principal(&self, auth_id: Uuid) -> Option<Principal> {
            self.principals.read().await.get(&auth_id).cloned()
        }
    }

    #[test]
    fn test_principal_auth_user_trait() {
        let auth_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let principal = Principal::new(auth_id, user_id, AuthMethod::Session);

        assert_eq!(AuthUser::id(&principal), auth_id);
        assert_eq!(principal.session_auth_hash(), auth_id.as_bytes());
    }

    #[tokio::test]
    async fn test_auth_backend_resolve_principal() {
        let auth_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let principal = Principal::new(auth_id, user_id, AuthMethod::Session);

        let backend = MockAuthBackend::default();
        assert_eq!(backend.resolve_principal(auth_id).await, None);

        backend
            .principals
            .write()
            .await
            .insert(auth_id, principal.clone());
        let resolved = backend.resolve_principal(auth_id).await;
        assert_eq!(resolved, Some(principal.clone()));

        // Also test through PrincipalAuthBackend (axum_login::AuthnBackend)
        let login_backend = PrincipalAuthBackend::new(backend);
        let user = login_backend
            .get_user(&auth_id)
            .await
            .expect("should not error");
        assert_eq!(user, Some(principal));
    }
}
