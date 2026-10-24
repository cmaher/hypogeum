use crate::AdminComponents;
use axum::{
    Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use exemplum_dmn_users::{
    accounts::{Account, AccountPage},
    models::PageRequest,
};
use hypgm_authn::RequireAuth;
use hypgm_http::{AppJson, HttpError};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
struct PageQuery {
    after: Option<Uuid>,
    limit: Option<u32>,
}

pub fn router(components: AdminComponents) -> Router<()> {
    Router::new()
        .route("/users", get(list))
        .route("/users/{id}/approve", post(approve))
        .with_state(components)
}
async fn list(
    State(components): State<AdminComponents>,
    RequireAuth(principal): RequireAuth,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<AppJson<AccountPage>, HttpError> {
    let Query(query) = query.map_err(|_| HttpError::invalid_request())?;
    let page = PageRequest::new(query.after, query.limit.unwrap_or(50)).map_err(HttpError::from)?;
    Ok(AppJson(components.service.list(&principal, page).await?))
}
async fn approve(
    State(components): State<AdminComponents>,
    RequireAuth(principal): RequireAuth,
    id: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> Result<AppJson<Account>, HttpError> {
    let Path(id) = id.map_err(|_| HttpError::invalid_request())?;
    Ok(AppJson(components.service.approve(&principal, id).await?))
}
