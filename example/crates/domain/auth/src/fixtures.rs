//! Explicit in-memory composition defaults for transport examples and tests.
use exemplum_dmn_users::{accounts::*, models::PageRequest};
pub struct ApprovedAccounts;
#[async_trait::async_trait]
impl AccountService for ApprovedAccounts {
    async fn register(&self, _: &str, _: &str, _: &str) -> Result<Account, AccountError> {
        Err(AccountError::Validation)
    }
    async fn approve(&self, _: uuid::Uuid) -> Result<Account, AccountError> {
        Err(AccountError::Validation)
    }
    async fn get(&self, id: uuid::Uuid) -> Result<Account, AccountError> {
        Ok(Account {
            id,
            display_name: "Fixture user".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            approval_status: ApprovalStatus::Approved,
            is_admin: false,
            approved_at: None,
        })
    }
    async fn list(&self, _: PageRequest) -> Result<AccountPage, AccountError> {
        Err(AccountError::Validation)
    }
}
