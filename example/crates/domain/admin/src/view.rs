//! Authorized server-rendered account administration.
use crate::AdminComponents;
use askama::Template;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use exemplum_dmn_users::{
    accounts::{AccountQuery, ApprovalStatus, ManagedAccount},
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
    filters: [Filter; 3],
    next: Option<String>,
    htmx: &'static str,
    script: &'static str,
}
#[derive(Template)]
#[template(path = "row.html")]
struct UserRow {
    id: Uuid,
    name: String,
    username: Option<String>,
    registered: String,
    pending: bool,
}
impl From<ManagedAccount> for UserRow {
    fn from(managed: ManagedAccount) -> Self {
        let account = managed.account;
        Self {
            id: account.id,
            name: account.display_name,
            username: managed.username,
            registered: account.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            pending: account.approval_status == ApprovalStatus::Pending,
        }
    }
}
/// A status filter link; the current filter renders as plain text.
struct Filter {
    label: &'static str,
    href: &'static str,
    current: bool,
}
fn filters(status: Option<ApprovalStatus>) -> [Filter; 3] {
    [
        (None, "All", "/admin/users"),
        (
            Some(ApprovalStatus::Pending),
            "Pending",
            "/admin/users?status=pending",
        ),
        (
            Some(ApprovalStatus::Approved),
            "Approved",
            "/admin/users?status=approved",
        ),
    ]
    .map(|(value, label, href)| Filter {
        label,
        href,
        current: value == status,
    })
}
fn status_param(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
    }
}
#[derive(Deserialize)]
struct ListQuery {
    after: Option<Uuid>,
    status: Option<ApprovalStatus>,
}
pub fn router(components: AdminComponents) -> Router<()> {
    Router::new()
        .route("/users", get(list))
        .route("/users/{id}/approve", axum::routing::post(approve))
        .with_state(components)
}
async fn list(
    State(components): State<AdminComponents>,
    auth: Result<RequireAuth, hypgm_authn::extractor::AuthRejection>,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, HtmlError> {
    let RequireAuth(principal) = auth.map_err(|e| HtmlError::from(e.0))?;
    let Query(query) =
        query.map_err(|_| HtmlError::from_http(hypgm_http::HttpError::invalid_request()))?;
    let status = query.status;
    let page = components
        .service
        .list(
            &principal,
            AccountQuery {
                page: PageRequest::new(query.after, 50).map_err(HtmlError::from)?,
                status,
            },
        )
        .await?;
    let next = page.next_after.map(|after| match status {
        Some(status) => format!("/admin/users?status={}&after={after}", status_param(status)),
        None => format!("/admin/users?after={after}"),
    });
    Ok(render_template(
        StatusCode::OK,
        &UsersPage {
            rows: page.accounts.into_iter().map(UserRow::from).collect(),
            filters: filters(status),
            next,
            htmx: hypgm_view::HTMX_SCRIPT_URL,
            script: hypgm_view::VIEW_SCRIPT_URL,
        },
    ))
}
async fn approve(
    State(components): State<AdminComponents>,
    auth: Result<RequireAuth, hypgm_authn::extractor::AuthRejection>,
    htmx: HtmxRequest,
    id: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> Result<Response, HtmlError> {
    let RequireAuth(principal) = auth.map_err(|e| HtmlError::from(e.0))?;
    let Path(id) =
        id.map_err(|_| HtmlError::from_http(hypgm_http::HttpError::invalid_request()))?;
    let account = components.service.approve(&principal, id).await?;
    if htmx.is_fragment() {
        Ok(render_template(StatusCode::OK, &UserRow::from(account)))
    } else {
        Ok(Redirect::to("/admin/users").into_response())
    }
}
