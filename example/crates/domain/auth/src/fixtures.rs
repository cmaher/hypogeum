//! Explicit in-memory composition defaults for transport examples and tests.
use async_trait::async_trait;
use axum::http::HeaderValue;
use exemplum_dmn_users::accounts::*;
use hypgmx_kratos::{Flow, FlowError, FlowKind, KratosError, Session, SessionCookie};
use uuid::Uuid;

use crate::{FlowClient, SessionVerifier};

/// Accounts that always exist and are approved.
pub struct ApprovedAccounts;

fn approved(id: Uuid, display_name: &str) -> Account {
    Account {
        id,
        display_name: display_name.into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        approval_status: ApprovalStatus::Approved,
        is_admin: false,
        approved_at: None,
    }
}

#[async_trait]
impl AccountService for ApprovedAccounts {
    async fn ensure_account(
        &self,
        id: Uuid,
        _: &str,
        display_name: &str,
    ) -> Result<Account, AccountError> {
        Ok(approved(id, display_name))
    }
    async fn approve(&self, _: Uuid) -> Result<ManagedAccount, AccountError> {
        Err(AccountError::Validation)
    }
    async fn get(&self, id: Uuid) -> Result<Account, AccountError> {
        Ok(approved(id, "Fixture user"))
    }
    async fn list(&self, _: AccountQuery) -> Result<AccountPage, AccountError> {
        Err(AccountError::Validation)
    }
}

/// An identity service where no session is valid and no flow exists.
pub struct NoSessions;

#[async_trait]
impl SessionVerifier for NoSessions {
    async fn whoami(&self, _: &SessionCookie) -> Result<Session, KratosError> {
        Err(KratosError::Unauthenticated)
    }
    async fn logout_url(&self, _: &SessionCookie) -> Result<String, KratosError> {
        Err(KratosError::Unauthenticated)
    }
}

#[async_trait]
impl FlowClient for NoSessions {
    async fn get_flow(
        &self,
        _: FlowKind,
        _: Uuid,
        _: Option<&HeaderValue>,
    ) -> Result<Flow, KratosError> {
        Err(KratosError::FlowGone)
    }
    async fn get_error(&self, _: Uuid) -> Result<FlowError, KratosError> {
        Err(KratosError::FlowGone)
    }
}
