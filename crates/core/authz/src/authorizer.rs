use cedar_policy::{
    Authorizer as CedarAuthorizer, Context, Decision, Entities, EntityId, EntityTypeName,
    EntityUid, PolicySet, Request, Schema,
};
use hypgm_authn::{AuthMethod, Principal};
use std::borrow::Cow;
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

impl<A: AsRef<str>, B: AsRef<str>> AsEntityUid for (A, B) {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError> {
        let type_name = EntityTypeName::from_str(self.0.as_ref()).map_err(|e| {
            AuthzError::EvaluationError(format!(
                "Invalid EntityTypeName '{}': {e}",
                self.0.as_ref()
            ))
        })?;
        let entity_id = EntityId::from_str(self.1.as_ref()).map_err(|e| {
            AuthzError::EvaluationError(format!("Invalid EntityId '{}': {e}", self.1.as_ref()))
        })?;
        Ok(EntityUid::from_type_name_and_id(type_name, entity_id))
    }
}

impl<A: AsRef<str>, B: AsRef<str>> AsEntityUid for &(A, B) {
    fn as_entity_uid(&self) -> Result<EntityUid, AuthzError> {
        (*self).as_entity_uid()
    }
}

/// In-memory policy authorization engine wrapping Cedar's authorizer, schema, and policy set.
#[derive(Clone, Debug)]
pub struct Authorizer {
    schema: Schema,
    policies: PolicySet,
    entities: Entities,
    cedar: CedarAuthorizer,
    principal_type: EntityTypeName,
}

impl Authorizer {
    /// Creates a new `Authorizer` from a validated `Schema` and `PolicySet`.
    pub fn new(schema: Schema, policies: PolicySet) -> Result<Self, AuthzError> {
        let entities = schema.action_entities().map_err(AuthzError::policy_load)?;
        Ok(Self {
            schema,
            policies,
            entities,
            cedar: CedarAuthorizer::new(),
            principal_type: EntityTypeName::from_str("User")
                .expect("User is a valid Cedar EntityTypeName"),
        })
    }

    /// Creates an authorizer with preloaded entities and schema-defined actions.
    pub fn new_with_entities(
        schema: Schema,
        policies: PolicySet,
        entities: Entities,
    ) -> Result<Self, AuthzError> {
        Self::new(schema, policies)?.with_entities(entities)
    }

    /// Replaces preloaded entities, preserving the schema-defined action hierarchy.
    pub fn with_entities(mut self, entities: Entities) -> Result<Self, AuthzError> {
        let entities = entities
            .upsert_entities(
                self.schema
                    .action_entities()
                    .map_err(AuthzError::policy_load)?
                    .iter()
                    .cloned(),
                None,
            )
            .map_err(AuthzError::policy_load)?;
        self.entities = Entities::from_entities(entities.iter().cloned(), Some(&self.schema))
            .map_err(AuthzError::policy_load)?;
        Ok(self)
    }

    /// Configures the Cedar principal `EntityTypeName` (defaults to `"User"`).
    pub fn with_principal_type(mut self, type_name: impl AsRef<str>) -> Result<Self, AuthzError> {
        let parsed = EntityTypeName::from_str(type_name.as_ref()).map_err(|e| {
            AuthzError::PolicyLoadError(format!(
                "Invalid principal EntityTypeName '{}': {e}",
                type_name.as_ref()
            ))
        })?;
        self.principal_type = parsed;
        Ok(self)
    }

    /// Returns a reference to the configured principal `EntityTypeName`.
    pub fn principal_type(&self) -> &EntityTypeName {
        &self.principal_type
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
        self.is_authorized_advanced(principal, action, resource, &self.entities, None)
    }

    /// Evaluates authorization with explicitly supplied entities.
    ///
    /// If preloaded base entities exist on this `Authorizer`, they are combined with `entities`.
    pub fn is_authorized_with_entities(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
        entities: &Entities,
    ) -> Decision {
        self.is_authorized_advanced(principal, action, resource, entities, None)
    }

    /// Evaluates authorization with additional execution context.
    pub fn is_authorized_with_context(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
        context: &Context,
    ) -> Decision {
        self.is_authorized_advanced(principal, action, resource, &self.entities, Some(context))
    }

    /// Evaluates authorization with both custom entities and execution context.
    pub fn is_authorized_advanced(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
        entities: &Entities,
        context: Option<&Context>,
    ) -> Decision {
        let principal_uid = match principal_to_entity_uid(principal, &self.principal_type) {
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

        let execution_context = match build_context(principal, context, &self.schema, &action_uid) {
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
            execution_context,
            Some(&self.schema),
        ) {
            Ok(req) => req,
            Err(err) => {
                tracing::warn!(error = %err, "Failed to construct Cedar Request; failing closed");
                return Decision::Deny;
            }
        };

        let eval_entities = match self.evaluation_entities(entities) {
            Ok(entities) => entities,
            Err(err) => {
                tracing::warn!(error = %err, "Failed to combine runtime entities; failing closed");
                return Decision::Deny;
            }
        };

        let response = self
            .cedar
            .is_authorized(&request, &self.policies, &eval_entities);

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

    fn evaluation_entities<'a>(
        &'a self,
        entities: &'a Entities,
    ) -> Result<Cow<'a, Entities>, AuthzError> {
        if self.entities.is_empty() {
            Ok(Cow::Borrowed(entities))
        } else if entities.is_empty() || std::ptr::eq(entities, &self.entities) {
            Ok(Cow::Borrowed(&self.entities))
        } else {
            self.entities
                .clone()
                .upsert_entities(entities.iter().cloned(), Some(&self.schema))
                .map(Cow::Owned)
                .map_err(AuthzError::evaluation)
        }
    }

    /// Authorizes a request, returning `Ok(())` on `Decision::Allow` and `Err(AuthzError::Forbidden)`
    /// on `Decision::Deny`.
    pub fn authorize(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
    ) -> Result<(), AuthzError> {
        self.authorize_advanced(principal, action, resource, &self.entities, None)
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
        self.authorize_advanced(principal, action, resource, entities, None)
    }

    /// Authorizes a request with additional execution context.
    pub fn authorize_with_context(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
        context: &Context,
    ) -> Result<(), AuthzError> {
        self.authorize_advanced(principal, action, resource, &self.entities, Some(context))
    }

    /// Authorizes a request with both custom entities and execution context.
    pub fn authorize_advanced(
        &self,
        principal: &Principal,
        action: impl AsEntityUid,
        resource: impl AsEntityUid,
        entities: &Entities,
        context: Option<&Context>,
    ) -> Result<(), AuthzError> {
        match self.is_authorized_advanced(principal, action, resource, entities, context) {
            Decision::Allow => Ok(()),
            Decision::Deny => Err(AuthzError::Forbidden),
        }
    }
}

fn principal_to_entity_uid(
    principal: &Principal,
    type_name: &EntityTypeName,
) -> Result<EntityUid, AuthzError> {
    let entity_id = EntityId::from_str(&principal.user_id.to_string()).map_err(|e| {
        AuthzError::EvaluationError(format!("Failed to parse principal EntityId: {e}"))
    })?;
    Ok(EntityUid::from_type_name_and_id(
        type_name.clone(),
        entity_id,
    ))
}

fn build_context(
    principal: &Principal,
    custom_context: Option<&Context>,
    schema: &Schema,
    action: &EntityUid,
) -> Result<Context, AuthzError> {
    let method_str = match principal.method {
        AuthMethod::BearerJwt => "bearer_jwt",
        AuthMethod::Session => "session",
        AuthMethod::ApiKey => "api_key",
    };
    let json_val = serde_json::json!({
        "auth_id": principal.auth_id.to_string(),
        "auth_method": method_str,
    });
    let base_ctx = Context::from_json_value(json_val, None)
        .map_err(|e| AuthzError::EvaluationError(format!("Failed to build Cedar context: {e}")))?;

    if custom_context
        .is_some_and(|ctx| ctx.get("auth_id").is_some() || ctx.get("auth_method").is_some())
    {
        return Err(AuthzError::EvaluationError(
            "Authentication context keys are reserved".into(),
        ));
    }

    // Project only framework-owned metadata onto the action's context contract.
    // Keep all caller-supplied fields in every candidate, so invalid custom data
    // never becomes valid through projection. Prefer including both metadata fields.
    for (auth_id, auth_method) in [(true, true), (true, false), (false, true), (false, false)] {
        let metadata = Context::from_pairs(base_ctx.clone().into_iter().filter(|(key, _)| {
            (key == "auth_id" && auth_id) || (key == "auth_method" && auth_method)
        }))
        .map_err(AuthzError::evaluation)?;
        let candidate = match custom_context {
            Some(custom) => metadata
                .merge(custom.clone())
                .map_err(AuthzError::evaluation)?,
            None => metadata,
        };
        if candidate.validate(schema, action).is_ok() {
            return Ok(candidate);
        }
    }
    Err(AuthzError::EvaluationError(
        "Execution context does not conform to action schema".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_schema_and_policies;

    #[test]
    fn ordinary_checks_borrow_the_preloaded_graph() {
        let (schema, policies) = load_schema_and_policies(
            r#"entity User; entity Document; action "View" appliesTo { principal: [User], resource: [Document] };"#,
            "permit(principal, action, resource);",
        ).unwrap();
        let authorizer = Authorizer::new(schema, policies).unwrap();
        // Ordinary and custom-context entry points pass this exact stored graph.
        let entities = authorizer
            .evaluation_entities(authorizer.entities())
            .unwrap();
        assert!(
            matches!(entities, Cow::Borrowed(graph) if std::ptr::eq(graph, authorizer.entities()))
        );
        let empty = Entities::empty();
        assert!(
            matches!(authorizer.evaluation_entities(&empty).unwrap(), Cow::Borrowed(graph) if std::ptr::eq(graph, authorizer.entities()))
        );
        let runtime = Entities::from_json_value(
            serde_json::json!([{
                "uid": { "type": "Document", "id": "runtime" }, "attrs": {}, "parents": []
            }]),
            None,
        )
        .unwrap();
        assert!(matches!(
            authorizer.evaluation_entities(&runtime).unwrap(),
            Cow::Owned(_)
        ));
    }
}
