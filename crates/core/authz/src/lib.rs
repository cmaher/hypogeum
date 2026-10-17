pub mod authorizer;
pub mod error;
pub mod guard;
pub mod loader;

pub use authorizer::{AsEntityUid, Authorizer};
pub use cedar_policy::{Context, Decision, Entities, EntityUid, PolicySet, Schema};
pub use error::AuthzError;
pub use guard::{
    AuthzRejection, PermissionPolicy, RequirePermission, require_permission,
    require_permission_advanced, require_permission_with_context, require_permission_with_entities,
};
pub use hypgm_errors::{PublicError, PublicErrorInfo};
pub use loader::{load_schema_and_policies, load_schema_and_policies_from_files};
