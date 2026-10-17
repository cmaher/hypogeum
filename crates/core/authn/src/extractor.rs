use crate::error::AuthError;
use crate::jwt::JwtValidator;
use crate::principal::Principal;
use crate::session::PrincipalAuthBackend;
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
        if info.status == axum::http::StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }
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

        let trimmed = auth_str.trim();
        if let Some((scheme, rest)) = trimmed.split_once(|c: char| c.is_ascii_whitespace()) {
            if scheme.eq_ignore_ascii_case("bearer") {
                let token = rest.trim();

                let principal = if let Some(validator) = parts.extensions.get::<JwtValidator>() {
                    validator.verify_token(token)?
                } else if let Some(validator) = parts.extensions.get::<Arc<JwtValidator>>() {
                    validator.verify_token(token)?
                } else {
                    return Err(AuthError::InvalidCredentials);
                };

                return Ok(Some(principal));
            }
        } else if trimmed.eq_ignore_ascii_case("bearer") {
            return Err(AuthError::MissingCredentials);
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
        .get::<axum_login::AuthSession<PrincipalAuthBackend>>()
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
            Ok(Some(principal)) => {
                parts.extensions.insert(principal.clone());
                Ok(RequireAuth(principal))
            }
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
            Ok(Some(principal)) => {
                parts.extensions.insert(principal.clone());
                Ok(OptionalAuth(Some(principal)))
            }
            Ok(None) => Ok(OptionalAuth(None)),
            Err(err) => Err(AuthRejection(err)),
        }
    }
}
