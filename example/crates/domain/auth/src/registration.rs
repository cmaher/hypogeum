//! Registration owns plaintext validation and bounded password work.
use crate::{
    api::{RegistrationRequest, RegistrationResponse},
    controller::AuthComponents,
};
use axum::{extract::State, http::StatusCode};
use hypgm_authn::password::hash_password;
use hypgm_http::{AppJson, HttpError};

pub async fn register(
    State(components): State<AuthComponents>,
    AppJson(request): AppJson<RegistrationRequest>,
) -> Result<(StatusCode, AppJson<RegistrationResponse>), HttpError> {
    Ok((
        StatusCode::CREATED,
        AppJson(components.register(request).await?),
    ))
}
impl AuthComponents {
    pub async fn register(
        &self,
        request: RegistrationRequest,
    ) -> Result<RegistrationResponse, HttpError> {
        let username = exemplum_dmn_users::accounts::normalize_username(&request.username)?;
        let name = request.display_name.trim();
        if name.is_empty()
            || name.chars().count() > 200
            || name.contains('\0')
            || !(12..=128).contains(&request.password.chars().count())
            || request.password.len() > 1024
        {
            return Err(HttpError::validation_error());
        }
        let permit = self
            .password_verifications
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HttpError::internal_error())?;
        let span = tracing::Span::current();
        let hash = tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            let _permit = permit;
            hash_password(&request.password)
        })
        .await
        .map_err(|_| HttpError::internal_error())??;
        let account = self.accounts.register(name, username, &hash).await?;
        Ok(RegistrationResponse {
            id: account.id,
            approval_status: account.approval_status,
        })
    }
}

pub async fn require_approved(
    accounts: &dyn exemplum_dmn_users::accounts::AccountService,
    user_id: uuid::Uuid,
) -> Result<bool, hypgm_authn::error::AuthError> {
    use exemplum_dmn_users::accounts::{AccountError, ApprovalStatus};
    match accounts.get(user_id).await {
        Ok(account) => Ok(account.approval_status == ApprovalStatus::Approved),
        Err(AccountError::Repo(hypgm_errors::RepoError::NotFound)) => Ok(false),
        Err(_) => Err(hypgm_authn::error::AuthError::Internal(
            "Account lookup failed".into(),
        )),
    }
}
