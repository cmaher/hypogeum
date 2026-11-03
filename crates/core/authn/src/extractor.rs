use crate::error::AuthError;
use crate::principal::Principal;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use hypgm_errors::PublicError;
use std::convert::Infallible;

/// Rejection for [`RequireAuth`]: a uniform JSON 401 response.
#[derive(Debug)]
pub struct AuthRejection(pub AuthError);

impl std::fmt::Display for AuthRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
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
        let body = serde_json::json!({ "code": info.code, "message": info.message });
        let mut response = (info.status, axum::Json(body)).into_response();
        response.extensions_mut().insert(info);
        response.extensions_mut().insert(info.status);
        response
    }
}

/// Requires the [`Principal`] an authentication middleware placed in request
/// extensions; rejects with 401 otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequireAuth(pub Principal);

/// The [`Principal`] from request extensions, or `None` for anonymous requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionalAuth(pub Option<Principal>);

impl<S: Send + Sync> FromRequestParts<S> for RequireAuth {
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Principal>()
            .copied()
            .map(RequireAuth)
            .ok_or(AuthRejection(AuthError::Unauthenticated))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for OptionalAuth {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(OptionalAuth(parts.extensions.get::<Principal>().copied()))
    }
}
