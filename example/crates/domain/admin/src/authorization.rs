use cedar_policy::Entities;
use exemplum_dmn_users::accounts::{Account, ApprovalStatus};
use hypgm_authn::Principal;
use hypgm_authz::{Authorizer, AuthzError};

pub const PRINCIPAL_TYPE: &str = "User";
pub const ACTION_TYPE: &str = "Action";
pub const RESOURCE_TYPE: &str = "Resource";
pub const ADMIN_USERS: &str = "admin_users";
pub const VIEW_ADMIN_USERS: &str = "view_admin_users";
pub const APPROVE_USER: &str = "approve_user";

pub fn authorize(
    authorizer: &Authorizer,
    principal: &Principal,
    account: &Account,
    action: &str,
) -> Result<(), AuthzError> {
    if account.id != principal.user_id {
        return Err(AuthzError::Forbidden);
    }
    // Framework principal_to_entity_uid maps User::<user_id>, not auth_id.
    let entities = Entities::from_json_value(serde_json::json!([{
        "uid": { "type": PRINCIPAL_TYPE, "id": principal.user_id.to_string() },
        "attrs": { "is_admin": account.is_admin, "approved": account.approval_status == ApprovalStatus::Approved },
        "parents": []
    }]), None).map_err(|_| AuthzError::Forbidden)?;
    authorizer.authorize_with_entities(
        principal,
        (ACTION_TYPE, action),
        (RESOURCE_TYPE, ADMIN_USERS),
        &entities,
    )
}
