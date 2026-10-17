use cedar_policy::{
    Authorizer as CedarAuthorizer, Context, Decision, Entities, EntityId, EntityTypeName,
    EntityUid, PolicySet, Request, Schema,
};
use hypgm_authn::{AuthMethod, Principal};
use std::str::FromStr;

use crate::error::AuthzError;

/// Trait for types that can be converted into a Cedar `EntityUid`.
pub trait AsEntityUid {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError>;
}

impl AsEntityUid for EntityUid {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError> {
        Ok(self.clone())
    }
}

impl AsEntityUid for &EntityUid {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError> {
        Ok((*self).clone())
    }
}

impl AsEntityUid for &str {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError> {
        EntityUid::from_str(self)
            .map_err(|e| AuthzError::EvaluationError(format!("Invalid EntityUid '{self}': {e}")))
    }
}

impl AsEntityUid for String {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError> {
        self.as_str().as_entity_uid()
    }
}

impl AsEntityUid for &String {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError> {
        self.as_str().as_entity_uid()
    }
}

/// In-memory policy authorization engine wrapping Cedar's authorizer, schema, and policy set.
#[derive(Clone, Debug)]
pub struct Authorizer {
    schema: Schema,
    policies: PolicySet,
    entities: Entities,
    cedar: CedarAuthorizer,
}

impl Authorizer {
    /// Creates a new `Authorizer` from a validated `Schema` and `PolicySet`.
    pub fn new(schema: Schema, policies: PolicySet) -> Self {
        Self {
            schema,
            policies,
            entities: Entities::empty(),
            cedar: CedarAuthorizer::new(),
        }
    }

    /// Creates a new `Authorizer` with additional preloaded entities.
    pub fn with_entities(schema: Schema, policies: PolicySet, entities: Entities) -> Self {
        Self {
            schema,
            policies,
            entities,
            cedar: CedarAuthorizer::new(),
        }
    }

    /// Returns a reference to the active `Schema`.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Returns a reference to the active `PolicySet`.
    pub fn policies(&self) -> &PolicySet {
        &self.policies
    }

    /// Returns a reference to the default `Entities`.
    pub fn entities(&self) -> &Entities {
        &self.entities
    }

    /// Evaluates authorization for `principal`, `action`, and `resource` using default entities.
    ///
    /// Follows strict fail-closed semantics: any missing attribute, syntax error, or evaluation
    /// error results in `Decision::Deny`.
    pub fn is_authorized(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
    ) -> Decision {
        self.is_authorized_with_entities(principal, action, resource, &self.entities)
    }

    /// Evaluates authorization with explicitly supplied entities.
    pub fn is_authorized_with_entities(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
        entities: &Entities,
    ) -> Decision {
        let principal_uid = match principal_to_entity_uid(principal) {
            Ok(uid) => uid,
            Err(err) => {
                tracing::warn!(error = %err, "Failed to map Principal to Cedar EntityUid; failing closed");
                return Decision::Deny;
            }
        };

        let action_uid = match action.as_entity_uid() {
            Ok(uid) => uid,
            Err(err) => {
                tracing::warn!(error = %err, "Invalid action EntityUid; failing closed");
                return Decision::Deny;
            }
        };

        let resource_uid = match resource.as_entity_uid() {
            Ok(uid) => uid,
            Err(err) => {
                tracing::warn!(error = %err, "Invalid resource EntityUid; failing closed");
                return Decision::Deny;
            }
        };

        let context = match build_context(principal) {
            Ok(ctx) => ctx,
            Err(err) => {
                tracing::warn!(error = %err, "Failed to build execution context; failing closed");
                return Decision::Deny;
            }
        };

        let request = match Request::new(
            principal_uid,
            action_uid,
            resource_uid,
            context,
            Some(&self.schema),
        ) {
            Ok(req) => req,
            Err(err) => {
                tracing::warn!(error = %err, "Failed to construct Cedar Request; failing closed");
                return Decision::Deny;
            }
        };

        let response = self.cedar.is_authorized(&request, &self.policies, entities);

        let diagnostics = response.diagnostics();
        let mut has_errors = false;
        for err in diagnostics.errors() {
            tracing::warn!(error = %err, "Cedar policy evaluation error; failing closed");
            has_errors = true;
        }

        if has_errors {
            return Decision::Deny;
        }

        response.decision()
    }

    /// Authorizes a request, returning `Ok(())` on `Decision::Allow` and `Err(AuthzError::Forbidden)`
    /// on `Decision::Deny`.
    pub fn authorize(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
    ) -> Result<(), AuthzError> {
        self.authorize_with_entities(principal, action, resource, &self.entities)
    }

    /// Authorizes a request with explicit entities, returning `Ok(())` on `Decision::Allow`
    /// and `Err(AuthzError::Forbidden)` on `Decision::Deny`.
    pub fn authorize_with_entities(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
        entities: &Entities,
    ) -> Result<(), AuthzError> {
        match self.is_authorized_with_entities(principal, action, resource, entities) {
            Decision::Allow => Ok(()),
            Decision::Deny => Err(AuthzError::Forbidden),
        }
    }
}

fn principal_to_entity_uid(principal: &Principal) -> Result<EntityUid, AuthzError> {
    let type_name = EntityTypeName::from_str("User").map_err(|e| {
        AuthzError::EvaluationError(format!("Failed to parse principal EntityTypeName: {e}"))
    })?;
    let entity_id = EntityId::from_str(&principal.user_id.to_string()).map_err(|e| {
        AuthzError::EvaluationError(format!("Failed to parse principal EntityId: {e}"))
    })?;
    Ok(EntityUid::from_type_name_and_id(type_name, entity_id))
}

fn build_context(principal: &Principal) -> Result<Context, AuthzError> {
    let method_str = match principal.method {
        AuthMethod::BearerJwt => "bearer_jwt",
        AuthMethod::Session => "session",
        AuthMethod::ApiKey => "api_key",
    };
    let json_val = serde_json::json!({
        "auth_id": principal.auth_id.to_string(),
        "auth_method": method_str,
    });
    Context::from_json_value(json_val, None)
        .map_err(|e| AuthzError::EvaluationError(format!("Failed to build Cedar context: {e}")))
}
