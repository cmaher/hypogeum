use crate::error::AuthError;
use crate::jwt::JwtValidator;
use crate::principal::Principal;
use crate::session::{AuthBackend, PrincipalAuthBackend};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use hypgm_errors::PublicError;
use std::ops::Deref;
use std::sync::Arc;

/// Error rejection returned when authentication fails.
/// Converts into a uniform JSON error response with HTTP status 401 Unauthorized or 500 Internal Server Error.
#[derive(Debug)]
pub struct AuthRejection(pub AuthError);

impl std::fmt::Display for AuthRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AuthRejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let info = self.0.public_error();
        let body = serde_json::json!({
            "code": info.code,
            "message": info.message,
        });
        let mut response = (info.status, axum::Json(body)).into_response();
        response.extensions_mut().insert(info);
        response.extensions_mut().insert(info.status);
        response
    }
}

/// Axum extractor that requires an authenticated [`Principal`].
/// Rejects with HTTP 401 Unauthorized if unauthenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequireAuth(pub Principal);

impl Deref for RequireAuth {
    type Target = Principal;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl RequireAuth {
    pub fn into_inner(self) -> Principal {
        self.0
    }
}

/// Axum extractor for optional authentication.
/// Yields `Some(Principal)` if authenticated, or `None` if anonymous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionalAuth(pub Option<Principal>);

impl Deref for OptionalAuth {
    type Target = Option<Principal>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl OptionalAuth {
    pub fn into_inner(self) -> Option<Principal> {
        self.0
    }
}

/// Helper function to inspect incoming request parts and extract a Principal.
pub fn extract_principal_from_parts(parts: &Parts) -> Result<Option<Principal>, AuthError> {
    // 1. Inspect Authorization: Bearer <token> header
    if let Some(auth_value) = parts.headers.get(header::AUTHORIZATION) {
        let auth_str = auth_value
            .to_str()
            .map_err(|_| AuthError::InvalidCredentials)?;

        if let Some(token) = auth_str
            .strip_prefix("Bearer ")
            .or_else(|| auth_str.strip_prefix("bearer "))
        {
            let token = token.trim();
            if token.is_empty() {
                return Err(AuthError::MissingCredentials);
            }

            let validator = parts
                .extensions
                .get::<JwtValidator>()
                .cloned()
                .or_else(|| {
                    parts
                        .extensions
                        .get::<Arc<JwtValidator>>()
                        .map(|v| (**v).clone())
                })
                .ok_or_else(|| {
                    AuthError::Internal("JwtValidator not found in request extensions".to_string())
                })?;

            let principal = validator.verify_token(token)?;
            return Ok(Some(principal));
        }
    }

    // 2. Fall back to active session
    // Check if Principal is directly in extensions (e.g. set by session middleware)
    if let Some(principal) = parts.extensions.get::<Principal>() {
        return Ok(Some(principal.clone()));
    }

    // Check if an axum-login AuthSession is present in extensions
    if let Some(principal) = parts
        .extensions
        .get::<axum_login::AuthSession<PrincipalAuthBackend<()>>>()
        .and_then(|s| s.user.as_ref())
    {
        return Ok(Some(principal.clone()));
    }

    if let Some(principal) = parts
        .extensions
        .get::<axum_login::AuthSession<PrincipalAuthBackend<Arc<dyn AuthBackend>>>>()
        .and_then(|s| s.user.as_ref())
    {
        return Ok(Some(principal.clone()));
    }

    Ok(None)
}

impl<S> FromRequestParts<S> for RequireAuth
where
    S: Send + Sync,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match extract_principal_from_parts(parts) {
            Ok(Some(principal)) => Ok(RequireAuth(principal)),
            Ok(None) => Err(AuthRejection(AuthError::MissingCredentials)),
            Err(err) => Err(AuthRejection(err)),
        }
    }
}

impl<S> FromRequestParts<S> for OptionalAuth
where
    S: Send + Sync,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match extract_principal_from_parts(parts) {
            Ok(principal_opt) => Ok(OptionalAuth(principal_opt)),
            Err(err) => Err(AuthRejection(err)),
        }
    }
}
