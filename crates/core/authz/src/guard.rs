use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use hypgm_authn::Principal;
use hypgm_http::render_public_error;

use crate::authorizer::{AsEntityUid, Authorizer};
use crate::error::AuthzError;

/// Error rejection returned when authorization fails.
///
/// Converts into a uniform JSON error response (e.g. HTTP 403 Forbidden with
/// `{"code": "forbidden", "message": "Access denied by policy"}`) using `hypgm-http`.
#[derive(Debug)]
pub struct AuthzRejection(pub AuthzError);

impl std::fmt::Display for AuthzRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AuthzRejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<AuthzError> for AuthzRejection {
    fn from(err: AuthzError) -> Self {
        Self(err)
    }
}

impl IntoResponse for AuthzRejection {
    fn into_response(self) -> Response {
        render_public_error(&self.0)
    }
}

/// Helper function to evaluate permissions against an `Authorizer`.
///
/// Returns `Ok(())` on `Decision::Allow`, or `Err(AuthzRejection(AuthzError::Forbidden))`
/// on `Decision::Deny`.
pub fn require_permission(
    authorizer: &Authorizer,
    principal: &Principal,
    action: impl AsEntityUid,
    resource: impl AsEntityUid,
) -> Result<(), AuthzRejection> {
    authorizer
        .authorize(principal, action, resource)
        .map_err(AuthzRejection)
}

/// Static declarative specification of action and resource for [`RequirePermission`].
pub trait PermissionPolicy: Send + Sync + 'static {
    fn action() -> &'static str;
    fn resource() -> &'static str;
}

/// Axum extractor that validates that the authenticated [`Principal`] has permission to execute
/// the specified action on the resource defined by [`PermissionPolicy`].
///
/// Requires `Arc<Authorizer>` to be registered in Axum `State` or request extensions,
/// and `Principal` (or `RequireAuth`) to be present in request extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirePermission<P: PermissionPolicy> {
    pub principal: Principal,
    _marker: PhantomData<P>,
}

impl<P: PermissionPolicy> Deref for RequirePermission<P> {
    type Target = Principal;

    fn deref(&self) -> &Self::Target {
        &self.principal
    }
}

impl<P: PermissionPolicy> RequirePermission<P> {
    pub fn into_inner(self) -> Principal {
        self.principal
    }

    /// Direct permission check helper.
    pub fn check(
        authorizer: &Authorizer,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
    ) -> Result<(), AuthzRejection> {
        require_permission(authorizer, principal, action, resource)
    }
}

impl<S, P> FromRequestParts<S> for RequirePermission<P>
where
    S: Send + Sync,
    P: PermissionPolicy,
    Arc<Authorizer>: FromRef<S>,
{
    type Rejection = AuthzRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let principal = if let Some(p) = parts.extensions.get::<Principal>() {
            p.clone()
        } else if let Some(auth) = parts.extensions.get::<hypgm_authn::RequireAuth>() {
            auth.0.clone()
        } else {
            return Err(AuthzRejection(AuthzError::Forbidden));
        };

        let authorizer = if let Some(authz) = parts.extensions.get::<Arc<Authorizer>>() {
            Arc::clone(authz)
        } else {
            Arc::<Authorizer>::from_ref(state)
        };

        require_permission(&authorizer, &principal, P::action(), P::resource())?;

        Ok(Self {
            principal,
            _marker: PhantomData,
        })
    }
}
