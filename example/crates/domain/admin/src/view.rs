//! Authorized server-rendered account administration.
use crate::AdminService;
use askama::Template;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use exemplum_dmn_users::{
    accounts::{Account, ApprovalStatus},
    models::PageRequest,
};
use hypgm_authn::RequireAuth;
use hypgm_view::{HtmlError, HtmxRequest, render_template};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Template)]
#[template(path = "users.html")]
struct UsersPage {
    rows: Vec<UserRow>,
    next: Option<Uuid>,
    htmx: &'static str,
    script: &'static str,
}
#[derive(Template)]
#[template(path = "row.html")]
struct UserRow {
    id: Uuid,
    name: String,
    pending: bool,
    processing: bool,
    completed: String,
}
impl From<Account> for UserRow {
    fn from(account: Account) -> Self {
        Self {
            id: account.id,
            name: account.display_name,
            pending: account.approval_status == ApprovalStatus::Pending,
            processing: account.approval_status == ApprovalStatus::Approved
                && account.welcome_scheduled_at.is_some()
                && account.welcome_processed_at.is_none(),
            completed: account
                .welcome_processed_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_default(),
        }
    }
}
#[derive(Deserialize)]
struct Pagination {
    after: Option<Uuid>,
}
pub fn router(service: AdminService) -> Router<()> {
    Router::new()
        .route("/users", get(list))
        .route("/users/{id}", get(row))
        .route("/users/{id}/approve", axum::routing::post(approve))
        .with_state(service)
}
async fn list(
    State(service): State<AdminService>,
    auth: Result<RequireAuth, hypgm_authn::extractor::AuthRejection>,
    query: Result<Query<Pagination>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, HtmlError> {
    let RequireAuth(principal) = auth.map_err(|e| HtmlError::from(e.0))?;
    let Query(query) =
        query.map_err(|_| HtmlError::from_http(hypgm_http::HttpError::invalid_request()))?;
    let page = service
        .list(
            &principal,
            PageRequest::new(query.after, 50).map_err(HtmlError::from)?,
        )
        .await
        .map_err(HtmlError::from_http)?;
    Ok(render_template(
        StatusCode::OK,
        &UsersPage {
            rows: page.accounts.into_iter().map(UserRow::from).collect(),
            next: page.next_after,
            htmx: hypgm_view::HTMX_SCRIPT_URL,
            script: hypgm_view::VIEW_SCRIPT_URL,
        },
    ))
}
async fn row(
    State(service): State<AdminService>,
    auth: Result<RequireAuth, hypgm_authn::extractor::AuthRejection>,
    id: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> Result<Response, HtmlError> {
    let RequireAuth(principal) = auth.map_err(|e| HtmlError::from(e.0))?;
    let Path(id) =
        id.map_err(|_| HtmlError::from_http(hypgm_http::HttpError::invalid_request()))?;
    let account = service
        .get(&principal, id)
        .await
        .map_err(HtmlError::from_http)?;
    Ok(render_template(StatusCode::OK, &UserRow::from(account)))
}
async fn approve(
    State(service): State<AdminService>,
    auth: Result<RequireAuth, hypgm_authn::extractor::AuthRejection>,
    htmx: HtmxRequest,
    id: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> Result<Response, HtmlError> {
    let RequireAuth(principal) = auth.map_err(|e| HtmlError::from(e.0))?;
    let Path(id) =
        id.map_err(|_| HtmlError::from_http(hypgm_http::HttpError::invalid_request()))?;
    let account = service
        .approve(&principal, id)
        .await
        .map_err(HtmlError::from_http)?;
    if htmx.is_fragment() {
        Ok(render_template(StatusCode::OK, &UserRow::from(account)))
    } else {
        Ok(Redirect::to("/admin/users").into_response())
    }
}
