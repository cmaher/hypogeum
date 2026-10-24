use crate::AdminService;
use axum::{
    Router,
    extract::Request,
    extract::{Path, Query, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
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

pub fn router(service: AdminService) -> Router<()> {
    Router::new()
        .route("/users", get(list))
        .route("/users/{id}/approve", post(approve))
        .layer(middleware::from_fn(same_origin))
        .with_state(service)
}
async fn same_origin(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    if !parts.method.is_safe() && hypgm_view::is_cross_site_request(&parts) {
        return HttpError::from(hypgm_authz::AuthzError::Forbidden).into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}
async fn list(
    State(service): State<AdminService>,
    RequireAuth(principal): RequireAuth,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<AppJson<AccountPage>, HttpError> {
    let Query(query) = query.map_err(|_| HttpError::invalid_request())?;
    let page = PageRequest::new(query.after, query.limit.unwrap_or(50)).map_err(HttpError::from)?;
    Ok(AppJson(service.list(&principal, page).await?))
}
async fn approve(
    State(service): State<AdminService>,
    RequireAuth(principal): RequireAuth,
    id: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> Result<AppJson<Account>, HttpError> {
    let Path(id) = id.map_err(|_| HttpError::invalid_request())?;
    Ok(AppJson(service.approve(&principal, id).await?))
}
