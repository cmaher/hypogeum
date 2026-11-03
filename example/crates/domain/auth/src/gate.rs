//! Resolves the identity-service session on every request.
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use exemplum_dmn_users::accounts::ApprovalStatus;
use hypgm_authn::Principal;
use hypgm_errors::PublicErrorInfo;
use hypgmx_kratos::{KratosError, session_cookie};

use crate::AuthComponents;

/// Request extension for a signed-in account that awaits approval. It carries
/// no authority: API routes see an anonymous request, and pages other than
/// [`PENDING_PAGES`] redirect to `/registration-pending`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingAccount;

/// Pages a pending account may use: its notice, logout and the flow pages.
const PENDING_PAGES: [&str; 6] = [
    "/registration-pending",
    "/logout",
    "/login",
    "/registration",
    "/settings",
    "/error",
];

enum Caller {
    Anonymous,
    Pending,
    Approved(Principal),
}

/// Axum middleware for every application route except health checks and
/// static assets, which it passes through untouched.
///
/// Without an `ory_kratos_session` cookie the request is anonymous. Otherwise
/// it asks Kratos for the session (forwarding only that cookie), provisions
/// the account on first use and inserts a [`Principal`] for approved accounts
/// or [`PendingAccount`] otherwise. Identity service or account failures fail
/// closed with a 500: JSON under `/api`, HTML elsewhere.
pub async fn session_gate(
    State(components): State<AuthComponents>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path == "/healthz" || path.starts_with(&format!("{}/", hypgm_view::assets::ASSETS_MOUNT)) {
        return next.run(request).await;
    }
    let api = path == "/api" || path.starts_with("/api/");
    let caller = match resolve(&components, request.headers()).await {
        Ok(caller) => caller,
        Err(()) if api => return hypgm_http::HttpError::internal_error().into_response(),
        Err(()) => return hypgm_view::html_error_response(PublicErrorInfo::INTERNAL_ERROR),
    };
    match caller {
        Caller::Anonymous => {}
        Caller::Approved(principal) => {
            request.extensions_mut().insert(principal);
        }
        Caller::Pending => {
            if !api && !PENDING_PAGES.contains(&request.uri().path()) {
                return Redirect::to("/registration-pending").into_response();
            }
            request.extensions_mut().insert(PendingAccount);
        }
    }
    next.run(request).await
}

async fn resolve(components: &AuthComponents, headers: &HeaderMap) -> Result<Caller, ()> {
    let Some(cookie) = session_cookie(headers) else {
        return Ok(Caller::Anonymous);
    };
    let session = match components.sessions.whoami(&cookie).await {
        Ok(session) => session,
        Err(KratosError::Unauthenticated) => return Ok(Caller::Anonymous),
        Err(error) => {
            tracing::error!(%error, "Session check failed");
            return Err(());
        }
    };
    let traits = &session.identity.traits;
    let (Some(username), Some(display_name)) =
        (traits["username"].as_str(), traits["display_name"].as_str())
    else {
        tracing::error!("Identity is missing username or display_name traits");
        return Err(());
    };
    let account = components
        .accounts
        .ensure_account(session.identity.id, username, display_name)
        .await
        .map_err(|error| tracing::error!(%error, "Account provisioning failed"))?;
    Ok(match account.approval_status {
        ApprovalStatus::Approved => Caller::Approved(Principal::new(session.id, account.id)),
        ApprovalStatus::Pending => Caller::Pending,
    })
}
