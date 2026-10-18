use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use hypgm_authn::{AuthError, Principal, extract_principal_from_parts};
use hypgm_errors::{PublicError, PublicErrorInfo};
use hypgm_http::render_public_error;

use crate::authorizer::{AsEntityUid, Authorizer};
use crate::error::AuthzError;

/// Error rejection returned when authentication or authorization fails.
///
/// Converts into a uniform JSON error response:
/// - HTTP 401 Unauthorized for missing or invalid authentication credentials (`hypgm-authn`).
/// - HTTP 403 Forbidden for policy denial (`AuthzError::Forbidden`).
/// - HTTP 500 Internal Server Error for evaluation or internal errors.
#[derive(Debug)]
pub enum AuthzRejection {
    Auth(AuthError),
    Authz(AuthzError),
}

impl std::fmt::Display for AuthzRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(err) => write!(f, "{err}"),
            Self::Authz(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AuthzRejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Auth(err) => Some(err),
            Self::Authz(err) => Some(err),
        }
    }
}

impl From<AuthError> for AuthzRejection {
    fn from(err: AuthError) -> Self {
        Self::Auth(err)
    }
}

impl From<AuthzError> for AuthzRejection {
    fn from(err: AuthzError) -> Self {
        Self::Authz(err)
    }
}

impl PublicError for AuthzRejection {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Auth(err) => err.public_error(),
            Self::Authz(err) => err.public_error(),
        }
    }
}

impl IntoResponse for AuthzRejection {
    fn into_response(self) -> Response {
        match self {
            Self::Auth(err) => hypgm_authn::AuthRejection(err).into_response(),
            Self::Authz(err) => render_public_error(&err),
        }
    }
}

/// Helper function to evaluate permissions against an `Authorizer`.
///
/// Returns `Ok(())` on `Decision::Allow`, or `Err(AuthzRejection::Authz(AuthzError::Forbidden))`
/// on `Decision::Deny`.
pub fn require_permission(
    authorizer: &Authorizer,
    principal: &Principal,
    action: impl AsEntityUid,
    resource: impl AsEntityUid,
) -> Result<(), AuthzRejection> {
    authorizer
        .authorize(principal, action, resource)
        .map_err(AuthzRejection::Authz)
}

/// Helper function to evaluate permissions against an `Authorizer` with explicitly supplied entities.
pub fn require_permission_with_entities(
    authorizer: &Authorizer,
    principal: &Principal,
    action: impl AsEntityUid,
    resource: impl AsEntityUid,
    entities: &cedar_policy::Entities,
) -> Result<(), AuthzRejection> {
    authorizer
        .authorize_with_entities(principal, action, resource, entities)
        .map_err(AuthzRejection::Authz)
}

/// Helper function to evaluate permissions against an `Authorizer` with additional execution context.
pub fn require_permission_with_context(
    authorizer: &Authorizer,
    principal: &Principal,
    action: impl AsEntityUid,
    resource: impl AsEntityUid,
    context: &cedar_policy::Context,
) -> Result<(), AuthzRejection> {
    authorizer
        .authorize_with_context(principal, action, resource, context)
        .map_err(AuthzRejection::Authz)
}

/// Helper function to evaluate permissions against an `Authorizer` with both custom entities and context.
pub fn require_permission_advanced(
    authorizer: &Authorizer,
    principal: &Principal,
    action: impl AsEntityUid,
    resource: impl AsEntityUid,
    entities: &cedar_policy::Entities,
    context: Option<&cedar_policy::Context>,
) -> Result<(), AuthzRejection> {
    authorizer
        .authorize_advanced(principal, action, resource, entities, context)
        .map_err(AuthzRejection::Authz)
}

/// Static declarative specification of action and resource for [`RequirePermission`].
///
/// Implementors specify the Cedar action and resource EntityUids as string literals,
/// for example `Action::"View"` and `Document::"public"`.
pub trait PermissionPolicy: Send + Sync + 'static {
    fn action() -> &'static str;
    fn resource() -> &'static str;
}

/// Axum extractor that validates that the caller is authenticated as a [`Principal`] and has permission
/// to execute the specified action on the resource defined by [`PermissionPolicy`].
///
/// Authenticates the caller via [`hypgm_authn::extract_principal_from_parts`] (inspecting Bearer JWT headers,
/// request extensions, and active sessions). Unauthenticated callers are rejected with HTTP 401 Unauthorized;
/// policy denials are rejected with HTTP 403 Forbidden.
///
/// Requires `Arc<Authorizer>` to be registered in Axum `State` (with optional per-request override in request extensions).
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

    /// Direct permission check helper for policy `P`.
    pub fn check(authorizer: &Authorizer, principal: &Principal) -> Result<(), AuthzRejection> {
        require_permission(authorizer, principal, P::action(), P::resource())
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
        let principal = match extract_principal_from_parts(parts) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return Err(AuthzRejection::Auth(AuthError::MissingCredentials));
            }
            Err(err) => return Err(AuthzRejection::Auth(err)),
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
