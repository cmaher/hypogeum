use cedar_policy::{PolicySet, Schema, ValidationMode, Validator};
use std::path::Path;
use std::str::FromStr;

use crate::error::AuthzError;

/// Parses and validates a Cedar schema and policy set from strings.
///
/// Ensures both that the syntax is valid and that the policies semantically adhere
/// to the schema (e.g. valid entity types, actions, and attributes).
pub fn load_schema_and_policies(
    schema_src: &str,
    policy_src: &str,
) -> Result<(Schema, PolicySet), AuthzError> {
    let schema = Schema::from_str(schema_src)
        .map_err(|e| AuthzError::PolicyLoadError(format!("Invalid Cedar schema: {e}")))?;

    let policies = PolicySet::from_str(policy_src)
        .map_err(|e| AuthzError::PolicyLoadError(format!("Invalid Cedar policy set: {e}")))?;

    let validator = Validator::new(schema.clone());
    let validation_result = validator.validate(&policies, ValidationMode::default());

    if !validation_result.validation_passed() {
        let err_msg = validation_result
            .validation_errors()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(AuthzError::PolicyLoadError(format!(
            "Policy validation failed against schema: {err_msg}"
        )));
    }

    Ok((schema, policies))
}

/// Loads and validates a Cedar schema and policy set from files on disk.
pub fn load_schema_and_policies_from_files(
    schema_path: impl AsRef<Path>,
    policy_path: impl AsRef<Path>,
) -> Result<(Schema, PolicySet), AuthzError> {
    let schema_p = schema_path.as_ref();
    let policy_p = policy_path.as_ref();

    let schema_src = std::fs::read_to_string(schema_p).map_err(|e| {
        AuthzError::PolicyLoadError(format!(
            "Failed to read schema file {}: {e}",
            schema_p.display()
        ))
    })?;
    let policy_src = std::fs::read_to_string(policy_p).map_err(|e| {
        AuthzError::PolicyLoadError(format!(
            "Failed to read policy file {}: {e}",
            policy_p.display()
        ))
    })?;

    load_schema_and_policies(&schema_src, &policy_src)
}
