use exemplum_dmn_users::accounts::{Account, ApprovalStatus};
use hypgm_authn::Principal;

/// Account administration requires an approved administrator acting as itself.
pub fn can_administer(principal: &Principal, account: &Account) -> bool {
    account.id == principal.user_id
        && account.is_admin
        && account.approval_status == ApprovalStatus::Approved
}
