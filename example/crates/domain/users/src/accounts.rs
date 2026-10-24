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
    /// Active password identity; `None` when the account has no password login.
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

/// A taken username is an ordinary form validation failure, not a resource conflict.
pub const USERNAME_UNAVAILABLE: PublicErrorInfo =
    PublicErrorInfo::validation_error("Username unavailable");

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Invalid account input")]
    Validation,
    #[error("Username unavailable")]
    DuplicateUsername,
    #[error(transparent)]
    Repo(#[from] RepoError),
}
impl PublicError for AccountError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Validation => PublicErrorInfo::VALIDATION_ERROR,
            Self::DuplicateUsername => USERNAME_UNAVAILABLE,
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
    /// Approve a pending account; approving an approved account changes nothing.
    async fn approve(&self, id: Uuid) -> Result<ManagedAccount, AccountError>;
    async fn get(&self, id: Uuid) -> Result<Account, AccountError>;
    /// List accounts by `created_at` descending, breaking ties by id.
    async fn list(&self, query: AccountQuery) -> Result<AccountPage, AccountError>;
    /// Create a new approved administrator. A username that already belongs to
    /// an account fails with `DuplicateUsername`; existing accounts change only
    /// through [`Self::promote_admin`].
    async fn create_admin(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError>;
    /// Explicitly make the account behind an active password identity an
    /// approved administrator. Credentials and profile data are never replaced.
    async fn promote_admin(&self, username: &str) -> Result<Account, AccountError>;
}
/// Explicit administrator bootstrap, kept apart from request-serving account
/// behavior so web composition never receives it.
#[async_trait]
pub trait AdminBootstrapService: Send + Sync {
    async fn create_admin(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError>;
    async fn promote_admin(&self, username: &str) -> Result<Account, AccountError>;
}
#[async_trait]
pub trait AccountService: Send + Sync {
    async fn register(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
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
pub const PASSWORD_MIN_CHARS: usize = 8;
pub const PASSWORD_MAX_CHARS: usize = 128;
/// Upper bound on a valid password's UTF-8 encoding, for bounded reads.
pub const PASSWORD_MAX_BYTES: usize = PASSWORD_MAX_CHARS * 4;
/// Plaintext password policy shared by registration and administrator creation.
pub fn validate_password(password: &str) -> Result<(), AccountError> {
    if !(PASSWORD_MIN_CHARS..=PASSWORD_MAX_CHARS).contains(&password.chars().count())
        || password.contains('\0')
    {
        return Err(AccountError::Validation);
    }
    Ok(())
}
/// Human-readable statement of [`validate_password`]'s policy.
pub fn password_policy() -> String {
    format!(
        "Password must contain {PASSWORD_MIN_CHARS} to {PASSWORD_MAX_CHARS} characters without NUL"
    )
}
/// Username matching remains case-sensitive after whitespace normalization.
pub fn normalize_username(username: &str) -> Result<&str, AccountError> {
    let username = username.trim();
    if username.is_empty() || username.chars().count() > 200 || username.contains('\0') {
        return Err(AccountError::Validation);
    }
    Ok(username)
}
/// Normalize and validate a new password account's profile and stored hash.
fn validate_new_account<'a>(
    display_name: &'a str,
    username: &'a str,
    password_hash: &str,
) -> Result<(&'a str, &'a str), AccountError> {
    let display_name = display_name.trim();
    validate_name(display_name).map_err(|_| AccountError::Validation)?;
    let username = normalize_username(username)?;
    if password_hash.is_empty() || password_hash.contains('\0') {
        return Err(AccountError::Validation);
    }
    Ok((display_name, username))
}

#[async_trait]
impl AccountService for DefaultAccountService {
    async fn register(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError> {
        let (display_name, username) = validate_new_account(display_name, username, password_hash)?;
        self.repo
            .register(display_name, username, password_hash)
            .await
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
    async fn create_admin(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError> {
        let (display_name, username) = validate_new_account(display_name, username, password_hash)?;
        self.repo
            .create_admin(display_name, username, password_hash)
            .await
    }
    async fn promote_admin(&self, username: &str) -> Result<Account, AccountError> {
        self.repo.promote_admin(normalize_username(username)?).await
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
        assert!(validate_password(&"界".repeat(128)).is_ok());
        assert!(validate_password(&"x".repeat(8)).is_ok());
        for bad in ["x".repeat(7), "x".repeat(129), "password\0".into()] {
            assert!(validate_password(&bad).is_err());
        }
        assert!(PageRequest::new(None, 0).is_err());
        assert!(PageRequest::new(None, 101).is_err());
        assert_eq!(
            AccountError::DuplicateUsername.public_error(),
            PublicErrorInfo::validation_error("Username unavailable")
        );
    }

    /// Records the normalized inputs that reach persistence.
    #[derive(Default)]
    struct RecordingRepo {
        calls: std::sync::Mutex<Vec<String>>,
    }
    impl RecordingRepo {
        fn account(&self, call: String) -> Result<Account, AccountError> {
            self.calls.lock().unwrap().push(call);
            Ok(Account {
                id: Uuid::nil(),
                display_name: "Admin".into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                approval_status: ApprovalStatus::Approved,
                is_admin: true,
                approved_at: Some(Utc::now()),
            })
        }
    }
    #[async_trait]
    impl AccountRepo for RecordingRepo {
        async fn register(&self, _: &str, _: &str, _: &str) -> Result<Account, AccountError> {
            unreachable!("bootstrap never registers")
        }
        async fn approve(&self, _: Uuid) -> Result<ManagedAccount, AccountError> {
            unreachable!("bootstrap never approves")
        }
        async fn get(&self, _: Uuid) -> Result<Account, AccountError> {
            unreachable!("bootstrap never reads")
        }
        async fn list(&self, _: AccountQuery) -> Result<AccountPage, AccountError> {
            unreachable!("bootstrap never lists")
        }
        async fn create_admin(
            &self,
            display_name: &str,
            username: &str,
            password_hash: &str,
        ) -> Result<Account, AccountError> {
            self.account(format!("create {display_name}|{username}|{password_hash}"))
        }
        async fn promote_admin(&self, username: &str) -> Result<Account, AccountError> {
            self.account(format!("promote {username}"))
        }
    }

    #[tokio::test]
    async fn admin_bootstrap_validates_and_normalizes_before_persistence() {
        let repo = Arc::new(RecordingRepo::default());
        let service = DefaultAccountService::new(repo.clone());
        service
            .create_admin(" Admin ", " admin ", "hash")
            .await
            .unwrap();
        service.promote_admin(" alice ").await.unwrap();
        for (name, username, hash) in [
            (" ", "admin", "hash"),
            ("Admin", " ", "hash"),
            ("Admin", "admin", ""),
        ] {
            assert!(matches!(
                service.create_admin(name, username, hash).await,
                Err(AccountError::Validation)
            ));
        }
        assert!(matches!(
            service.promote_admin(" ").await,
            Err(AccountError::Validation)
        ));
        assert_eq!(
            *repo.calls.lock().unwrap(),
            ["create Admin|admin|hash", "promote alice"]
        );
    }
}
