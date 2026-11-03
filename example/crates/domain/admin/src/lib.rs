//! Authorized example account administration shared by JSON and HTML adapters.
//! Cross-site write protection is owned by the composition root.
pub mod api;
pub mod authorization;
pub mod view;

use async_trait::async_trait;
use exemplum_dmn_users::accounts::{
    AccountError, AccountPage, AccountQuery, AccountService, ManagedAccount,
};
use hypgm_authn::Principal;
use hypgm_errors::{PublicError, PublicErrorInfo, RepoError};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("Access denied")]
    Forbidden,
    #[error(transparent)]
    Account(#[from] AccountError),
}
impl PublicError for AdminError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Forbidden => PublicErrorInfo::FORBIDDEN,
            Self::Account(error) => error.public_error(),
        }
    }
}

/// Account administration that authorizes every call against current authority.
#[async_trait]
pub trait AdminService: Send + Sync {
    async fn list(
        &self,
        principal: &Principal,
        query: AccountQuery,
    ) -> Result<AccountPage, AdminError>;
    async fn approve(&self, principal: &Principal, id: Uuid) -> Result<ManagedAccount, AdminError>;
}

pub struct DefaultAdminService {
    accounts: Arc<dyn AccountService>,
}
impl DefaultAdminService {
    pub fn new(accounts: Arc<dyn AccountService>) -> Self {
        Self { accounts }
    }
    async fn authorize(&self, principal: &Principal) -> Result<(), AdminError> {
        // Always reload authority: identity sessions never supply roles.
        let account = match self.accounts.get(principal.user_id).await {
            Ok(account) => account,
            // A principal without an account has no authority; lookup failures stay visible.
            Err(AccountError::Repo(RepoError::NotFound)) => {
                return Err(AdminError::Forbidden);
            }
            Err(error) => return Err(error.into()),
        };
        if !authorization::can_administer(principal, &account) {
            return Err(AdminError::Forbidden);
        }
        Ok(())
    }
}
#[async_trait]
impl AdminService for DefaultAdminService {
    async fn list(
        &self,
        principal: &Principal,
        query: AccountQuery,
    ) -> Result<AccountPage, AdminError> {
        self.authorize(principal).await?;
        Ok(self.accounts.list(query).await?)
    }
    async fn approve(&self, principal: &Principal, id: Uuid) -> Result<ManagedAccount, AdminError> {
        self.authorize(principal).await?;
        Ok(self.accounts.approve(id).await?)
    }
}

#[derive(Clone)]
pub struct AdminComponents {
    pub service: Arc<dyn AdminService>,
}
impl AdminComponents {
    pub fn new(service: Arc<dyn AdminService>) -> Self {
        Self { service }
    }
}
