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
    pub welcome_processed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountPage {
    pub accounts: Vec<Account>,
    pub next_after: Option<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Invalid account input")]
    Validation,
    #[error("Username already registered")]
    DuplicateUsername,
    #[error(transparent)]
    Repo(#[from] RepoError),
}
impl PublicError for AccountError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Validation => PublicErrorInfo::VALIDATION_ERROR,
            Self::DuplicateUsername => PublicErrorInfo::CONFLICT,
            Self::Repo(error) => error.public_error(),
        }
    }
}

/// Persistence accepts a precomputed password hash, never plaintext credentials.
#[async_trait]
pub trait AccountRepo: Send + Sync {
    async fn register(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError>;
    async fn get(&self, id: Uuid) -> Result<Account, AccountError>;
    async fn list(&self, page: PageRequest) -> Result<AccountPage, AccountError>;
}
#[async_trait]
pub trait AccountService: Send + Sync {
    async fn register(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError>;
    async fn get(&self, id: Uuid) -> Result<Account, AccountError>;
    async fn list(&self, page: PageRequest) -> Result<AccountPage, AccountError>;
}
pub struct DefaultAccountService {
    repo: Arc<dyn AccountRepo>,
}
impl DefaultAccountService {
    pub fn new(repo: Arc<dyn AccountRepo>) -> Self {
        Self { repo }
    }
}
/// Username matching remains case-sensitive after whitespace normalization.
pub fn normalize_username(username: &str) -> Result<&str, AccountError> {
    let username = username.trim();
    if username.is_empty() || username.chars().count() > 200 || username.contains('\0') {
        return Err(AccountError::Validation);
    }
    Ok(username)
}

#[async_trait]
impl AccountService for DefaultAccountService {
    async fn register(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError> {
        let display_name = display_name.trim();
        validate_name(display_name).map_err(|_| AccountError::Validation)?;
        let username = normalize_username(username)?;
        if password_hash.is_empty() || password_hash.contains('\0') {
            return Err(AccountError::Validation);
        }
        self.repo
            .register(display_name, username, password_hash)
            .await
    }
    async fn get(&self, id: Uuid) -> Result<Account, AccountError> {
        self.repo.get(id).await
    }
    async fn list(&self, page: PageRequest) -> Result<AccountPage, AccountError> {
        self.repo.list(page).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usernames_are_trimmed_bounded_and_case_sensitive() {
        assert_eq!(normalize_username(" Alice ").unwrap(), "Alice");
        for bad in ["", " ", "bad\0name", &"界".repeat(201)] {
            assert!(normalize_username(bad).is_err());
        }
        assert!(normalize_username(&"界".repeat(200)).is_ok());
        assert!(PageRequest::new(None, 0).is_err());
        assert!(PageRequest::new(None, 101).is_err());
        assert_eq!(
            AccountError::DuplicateUsername.public_error(),
            PublicErrorInfo::CONFLICT
        );
    }
}
