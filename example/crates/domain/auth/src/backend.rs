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
    let (parts, body) = request.into_parts();
    match validate_bearer(&backend, &parts).await {
        Ok(()) => {
            next.run(axum::extract::Request::from_parts(parts, body))
                .await
        }
        Err(error) => hypgm_authn::extractor::AuthRejection(error).into_response(),
    }
}

/// Revalidate bearer identities while preserving the page router's HTML errors.
pub async fn html_bearer_gate(
    axum::extract::State(backend): axum::extract::State<ExemplumAuthBackend>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let (parts, body) = request.into_parts();
    match validate_bearer(&backend, &parts).await {
        Ok(()) => {
            next.run(axum::extract::Request::from_parts(parts, body))
                .await
        }
        Err(error) => {
            let mut response = hypgm_view::HtmlError::from(error).into_response();
            if response.status() == axum::http::StatusCode::UNAUTHORIZED {
                response.headers_mut().insert(
                    axum::http::header::WWW_AUTHENTICATE,
                    axum::http::HeaderValue::from_static("Bearer"),
                );
            }
            response
        }
    }
}

/// Uses the framework extractor so the gate sees exactly the bearer principal
/// that `RequireAuth` will accept. Extraction failures pass through for the
/// extractor to reject; session principals are already revalidated on load.
async fn validate_bearer(
    backend: &ExemplumAuthBackend,
    parts: &axum::http::request::Parts,
) -> Result<(), AuthError> {
    if let Ok(Some(principal)) = hypgm_authn::extractor::extract_principal_from_parts(parts)
        && principal.method == hypgm_authn::AuthMethod::BearerJwt
    {
        match backend.resolve_principal(principal.auth_id).await {
            Ok(Some(current)) if current.user_id == principal.user_id => {}
            Ok(_) => return Err(AuthError::InvalidCredentials),
            Err(error) => return Err(error),
        }
    }
    Ok(())
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
