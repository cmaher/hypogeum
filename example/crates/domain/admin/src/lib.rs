//! Authorized example account administration shared by JSON and HTML adapters.
pub mod api;
pub mod authorization;

use exemplum_dmn_users::{
    accounts::{Account, AccountPage, AccountService},
    models::PageRequest,
};
use hypgm_authn::Principal;
use hypgm_authz::Authorizer;
use hypgm_http::HttpError;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct AdminService {
    accounts: Arc<dyn AccountService>,
    authorizer: Arc<Authorizer>,
}
impl AdminService {
    pub fn new(accounts: Arc<dyn AccountService>, authorizer: Arc<Authorizer>) -> Self {
        Self {
            accounts,
            authorizer,
        }
    }
    async fn authorize(&self, principal: &Principal, action: &str) -> Result<(), HttpError> {
        // Always reload authority: sessions and JWT claims never supply roles.
        let account = self
            .accounts
            .get(principal.user_id)
            .await
            .map_err(|_| HttpError::from(hypgm_authz::AuthzError::Forbidden))?;
        authorization::authorize(&self.authorizer, principal, &account, action)
            .map_err(HttpError::from)
    }
    pub async fn list(
        &self,
        principal: &Principal,
        page: PageRequest,
    ) -> Result<AccountPage, HttpError> {
        self.authorize(principal, authorization::VIEW_ADMIN_USERS)
            .await?;
        self.accounts.list(page).await.map_err(HttpError::from)
    }
    pub async fn get(&self, principal: &Principal, id: Uuid) -> Result<Account, HttpError> {
        self.authorize(principal, authorization::VIEW_ADMIN_USERS)
            .await?;
        self.accounts.get(id).await.map_err(HttpError::from)
    }
    pub async fn approve(&self, principal: &Principal, id: Uuid) -> Result<Account, HttpError> {
        self.authorize(principal, authorization::APPROVE_USER)
            .await?;
        self.accounts.approve(id).await.map_err(HttpError::from)
    }
}

pub mod view;
