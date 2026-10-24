use async_trait::async_trait;
use hypgm_authn::{
    error::AuthError,
    principal::{AuthMethod, Principal},
    session::AuthBackend,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::models::AuthIdentity;
use crate::repo::AuthRepo;

#[derive(Clone)]
pub struct ExemplumAuthBackend {
    repo: Arc<dyn AuthRepo>,
    accounts: Arc<dyn exemplum_dmn_users::accounts::AccountService>,
}

impl ExemplumAuthBackend {
    pub fn from_arc(
        repo: Arc<dyn AuthRepo>,
        accounts: Arc<dyn exemplum_dmn_users::accounts::AccountService>,
    ) -> Self {
        Self { repo, accounts }
    }

    pub fn repo(&self) -> &Arc<dyn AuthRepo> {
        &self.repo
    }
}

#[async_trait]
impl AuthBackend for ExemplumAuthBackend {
    async fn resolve_principal(&self, auth_id: Uuid) -> Result<Option<Principal>, AuthError> {
        let identity: Option<AuthIdentity> = self
            .repo
            .find_active_by_id(auth_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        match identity {
            Some(ident) => {
                if ident.revoked_at.is_some()
                    || !crate::registration::require_approved(self.accounts.as_ref(), ident.user_id)
                        .await?
                {
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

/// Revalidate bearer identities against current database account state.
pub async fn bearer_gate(
    axum::extract::State(backend): axum::extract::State<ExemplumAuthBackend>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let authorization = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if let Some((scheme, token)) =
        authorization.and_then(|value| value.trim().split_once(char::is_whitespace))
        && scheme.eq_ignore_ascii_case("bearer")
        && let Some(validator) = request.extensions().get::<hypgm_authn::jwt::JwtValidator>()
        && let Ok(principal) = validator.verify_token(token.trim())
    {
        match backend.resolve_principal(principal.auth_id).await {
            Ok(Some(current)) if current.user_id == principal.user_id => {}
            Ok(_) => {
                return hypgm_authn::extractor::AuthRejection(AuthError::InvalidCredentials)
                    .into_response();
            }
            Err(error) => return hypgm_authn::extractor::AuthRejection(error).into_response(),
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::InMemoryAuthRepo;

    #[tokio::test]
    async fn test_resolve_active_principal() {
        let repo = Arc::new(InMemoryAuthRepo::new());
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

        let backend =
            ExemplumAuthBackend::from_arc(repo, Arc::new(crate::fixtures::ApprovedAccounts));
        let principal = backend.resolve_principal(auth_id).await.unwrap().unwrap();

        assert_eq!(principal.auth_id, auth_id);
        assert_eq!(principal.user_id, user_id);
        assert_eq!(principal.method, AuthMethod::Session);
    }

    #[tokio::test]
    async fn test_resolve_revoked_principal_fails() {
        let repo = Arc::new(InMemoryAuthRepo::new());
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

        let backend =
            ExemplumAuthBackend::from_arc(repo, Arc::new(crate::fixtures::ApprovedAccounts));
        let principal = backend.resolve_principal(auth_id).await.unwrap();
        assert!(principal.is_none());
    }

    #[tokio::test]
    async fn test_resolve_nonexistent_principal() {
        let repo = Arc::new(InMemoryAuthRepo::new());
        let backend =
            ExemplumAuthBackend::from_arc(repo, Arc::new(crate::fixtures::ApprovedAccounts));
        let principal = backend.resolve_principal(Uuid::new_v4()).await.unwrap();
        assert!(principal.is_none());
    }
}
