//! Example-owned account lifecycle, separate from generic user CRUD.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hypgm_errors::{PublicError, PublicErrorInfo, RepoError};
use sea_orm::prelude::StringLen;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::models::{PageRequest, validate_name};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    sea_orm::EnumIter,
    sea_orm::DeriveActiveEnum,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "approved")]
    Approved,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: Uuid,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub approval_status: ApprovalStatus,
    pub is_admin: bool,
    pub approved_at: Option<DateTime<Utc>>,
}

/// An account with the login username an administrator is approving.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedAccount {
    #[serde(flatten)]
    pub account: Account,
    /// Identity username; `None` for users created outside the identity service.
    pub username: Option<String>,
}

/// Newest-first account listing, optionally narrowed to one approval status.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccountQuery {
    pub page: PageRequest,
    pub status: Option<ApprovalStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountPage {
    pub accounts: Vec<ManagedAccount>,
    /// Cursor for the next page: the id of the last account on this page.
    pub next_after: Option<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Invalid account input")]
    Validation,
    #[error(transparent)]
    Repo(#[from] RepoError),
}
impl PublicError for AccountError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Validation => PublicErrorInfo::VALIDATION_ERROR,
            Self::Repo(error) => error.public_error(),
        }
    }
}

#[async_trait]
pub trait AccountRepo: Send + Sync {
    /// Insert a pending account for an identity unless one already exists,
    /// then return the stored account. Existing accounts are never changed.
    async fn ensure_account(
        &self,
        id: Uuid,
        username: &str,
        display_name: &str,
    ) -> Result<Account, AccountError>;
    /// Approve a pending account; approving an approved account changes nothing.
    async fn approve(&self, id: Uuid) -> Result<ManagedAccount, AccountError>;
    async fn get(&self, id: Uuid) -> Result<Account, AccountError>;
    /// List accounts by `created_at` descending, breaking ties by id.
    async fn list(&self, query: AccountQuery) -> Result<AccountPage, AccountError>;
    /// Make the account with this username an approved administrator.
    async fn promote_admin(&self, username: &str) -> Result<Account, AccountError>;
}
/// Explicit administrator bootstrap, kept apart from request-serving account
/// behavior so web composition never receives it.
#[async_trait]
pub trait AdminBootstrapService: Send + Sync {
    async fn promote_admin(&self, username: &str) -> Result<Account, AccountError>;
}
#[async_trait]
pub trait AccountService: Send + Sync {
    /// Lazily provision the account for an authenticated identity.
    async fn ensure_account(
        &self,
        id: Uuid,
        username: &str,
        display_name: &str,
    ) -> Result<Account, AccountError>;
    async fn approve(&self, id: Uuid) -> Result<ManagedAccount, AccountError>;
    async fn get(&self, id: Uuid) -> Result<Account, AccountError>;
    async fn list(&self, query: AccountQuery) -> Result<AccountPage, AccountError>;
}
pub struct DefaultAccountService {
    repo: Arc<dyn AccountRepo>,
}
impl DefaultAccountService {
    pub fn new(repo: Arc<dyn AccountRepo>) -> Self {
        Self { repo }
    }
}
/// Usernames follow the identity schema: 3 to 64 characters from
/// `[a-z0-9_.-]`, starting with a letter or digit.
pub fn validate_username(username: &str) -> Result<(), AccountError> {
    let valid = (3..=64).contains(&username.len())
        && username.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && username
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(AccountError::Validation)
    }
}

#[async_trait]
impl AccountService for DefaultAccountService {
    async fn ensure_account(
        &self,
        id: Uuid,
        username: &str,
        display_name: &str,
    ) -> Result<Account, AccountError> {
        validate_username(username)?;
        validate_name(display_name).map_err(|_| AccountError::Validation)?;
        self.repo.ensure_account(id, username, display_name).await
    }
    async fn approve(&self, id: Uuid) -> Result<ManagedAccount, AccountError> {
        self.repo.approve(id).await
    }
    async fn get(&self, id: Uuid) -> Result<Account, AccountError> {
        self.repo.get(id).await
    }
    async fn list(&self, query: AccountQuery) -> Result<AccountPage, AccountError> {
        self.repo.list(query).await
    }
}
#[async_trait]
impl AdminBootstrapService for DefaultAccountService {
    async fn promote_admin(&self, username: &str) -> Result<Account, AccountError> {
        validate_username(username)?;
        self.repo.promote_admin(username).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usernames_follow_the_identity_schema() {
        for good in ["abc", "a.b-c_d", "0admin", &"a".repeat(64)] {
            assert!(validate_username(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "ab",
            "Alice",
            "_abc",
            " abc",
            "ab c",
            &"a".repeat(65),
            "ab\0c",
        ] {
            assert!(validate_username(bad).is_err(), "{bad}");
        }
    }
}
