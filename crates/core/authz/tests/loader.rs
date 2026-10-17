use hypgm_authz::{AuthzError, load_schema_and_policies, load_schema_and_policies_from_files};

const VALID_SCHEMA: &str = r#"
entity User;
entity Document;
action "View" appliesTo {
    principal: [User],
    resource: [Document]
};
"#;

const VALID_POLICY: &str = r#"
permit(
    principal,
    action == Action::"View",
    resource
);
"#;

const INVALID_SYNTAX_SCHEMA: &str = r#"
entity User { invalid syntax !!!
"#;

const INVALID_SYNTAX_POLICY: &str = r#"
permit(principal, action == 1234 invalid syntax);
"#;

const INVALID_ENTITY_POLICY: &str = r#"
permit(
    principal == SuperUser::"admin",
    action == Action::"View",
    resource
);
"#;

const UNDEFINED_ACTION_POLICY: &str = r#"
permit(
    principal,
    action == Action::"NonExistentAction",
    resource
);
"#;

#[test]
fn test_load_valid_schema_and_policies() {
    let result = load_schema_and_policies(VALID_SCHEMA, VALID_POLICY);
    assert!(result.is_ok(), "Expected valid loading, got: {:?}", result.err());
}

#[test]
fn test_load_invalid_schema_syntax() {
    let result = load_schema_and_policies(INVALID_SYNTAX_SCHEMA, VALID_POLICY);
    match result {
        Err(AuthzError::PolicyLoadError(msg)) => {
            assert!(msg.contains("Invalid Cedar schema"));
        }
        other => panic!("Expected PolicyLoadError for invalid schema syntax, got {other:?}"),
    }
}

#[test]
fn test_load_invalid_policy_syntax() {
    let result = load_schema_and_policies(VALID_SCHEMA, INVALID_SYNTAX_POLICY);
    match result {
        Err(AuthzError::PolicyLoadError(msg)) => {
            assert!(msg.contains("Invalid Cedar policy set"));
        }
        other => panic!("Expected PolicyLoadError for invalid policy syntax, got {other:?}"),
    }
}

#[test]
fn test_load_policy_with_undefined_entity_type_fails_validation() {
    let result = load_schema_and_policies(VALID_SCHEMA, INVALID_ENTITY_POLICY);
    match result {
        Err(AuthzError::PolicyLoadError(msg)) => {
            assert!(
                msg.contains("validation failed against schema")
                    || msg.contains("SuperUser"),
                "Expected validation error message, got: {msg}"
            );
        }
        other => panic!("Expected PolicyLoadError for undefined entity type, got {other:?}"),
    }
}

#[test]
fn test_load_policy_with_undefined_action_fails_validation() {
    let result = load_schema_and_policies(VALID_SCHEMA, UNDEFINED_ACTION_POLICY);
    match result {
        Err(AuthzError::PolicyLoadError(msg)) => {
            assert!(
                msg.contains("validation failed against schema")
                    || msg.contains("NonExistentAction"),
                "Expected validation error message, got: {msg}"
            );
        }
        other => panic!("Expected PolicyLoadError for undefined action, got {other:?}"),
    }
}

#[test]
fn test_load_from_files() {
    let dir = std::env::temp_dir().join(format!("hypgm_authz_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    let schema_file = dir.join("policy.cedarschema");
    let policy_file = dir.join("policy.cedar");

    std::fs::write(&schema_file, VALID_SCHEMA).unwrap();
    std::fs::write(&policy_file, VALID_POLICY).unwrap();

    let res = load_schema_and_policies_from_files(&schema_file, &policy_file);
    assert!(res.is_ok());

    // Non-existent file fails with PolicyLoadError
    let bad_file = dir.join("does_not_exist.cedar");
    let bad_res = load_schema_and_policies_from_files(&schema_file, &bad_file);
    match bad_res {
        Err(AuthzError::PolicyLoadError(msg)) => {
            assert!(msg.contains("Failed to read policy file"));
        }
        other => panic!("Expected PolicyLoadError for missing file, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
