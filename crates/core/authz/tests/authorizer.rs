use hypgm_authn::{AuthMethod, Principal};
use hypgm_authz::{Authorizer, AuthzError, Decision, load_schema_and_policies};
use uuid::Uuid;

const SCHEMA: &str = r#"
entity User;
entity Document;
action "View" appliesTo {
    principal: [User],
    resource: [Document],
    context: {
        auth_id: String,
        auth_method: String
    }
};
action "Edit" appliesTo {
    principal: [User],
    resource: [Document],
    context: {
        auth_id: String,
        auth_method: String
    }
};
action "Delete" appliesTo {
    principal: [User],
    resource: [Document],
    context: {
        auth_id: String,
        auth_method: String
    }
};
"#;

const POLICIES: &str = r#"
// Permit View for any user if using bearer_jwt
permit(
    principal,
    action == Action::"View",
    resource
) when {
    context.auth_method == "bearer_jwt"
};

// Permit Edit if using session
permit(
    principal,
    action == Action::"Edit",
    resource
) when {
    context.auth_method == "session"
};

// Explicit forbid for a specific banned user ID
forbid(
    principal == User::"00000000-0000-0000-0000-000000000001",
    action,
    resource
);
"#;

fn create_test_authorizer() -> Authorizer {
    let (schema, policies) = load_schema_and_policies(SCHEMA, POLICIES)
        .expect("Test schema and policies must load successfully");
    Authorizer::new(schema, policies)
}

#[test]
fn test_authorizer_permit_decision() {
    let authorizer = create_test_authorizer();
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);

    let decision =
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-123\"");
    assert_eq!(decision, Decision::Allow);

    let auth_res = authorizer.authorize(&principal, "Action::\"View\"", "Document::\"doc-123\"");
    assert!(auth_res.is_ok());
}

#[test]
fn test_authorizer_default_deny() {
    let authorizer = create_test_authorizer();
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);

    // Delete has no permit policy -> default deny
    let decision =
        authorizer.is_authorized(&principal, "Action::\"Delete\"", "Document::\"doc-123\"");
    assert_eq!(decision, Decision::Deny);

    let auth_res = authorizer.authorize(&principal, "Action::\"Delete\"", "Document::\"doc-123\"");
    match auth_res {
        Err(AuthzError::Forbidden) => {}
        other => panic!("Expected Forbidden error on default deny, got {other:?}"),
    }
}

#[test]
fn test_authorizer_explicit_forbid() {
    let authorizer = create_test_authorizer();
    let banned_user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let auth_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, banned_user_id, AuthMethod::BearerJwt);

    // View would normally be permitted, but explicit forbid matches
    let decision =
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-123\"");
    assert_eq!(decision, Decision::Deny);

    let auth_res = authorizer.authorize(&principal, "Action::\"View\"", "Document::\"doc-123\"");
    assert!(matches!(auth_res, Err(AuthzError::Forbidden)));
}

#[test]
fn test_authorizer_context_method_mismatch_denied() {
    let authorizer = create_test_authorizer();
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    // Edit requires Session, but we pass ApiKey
    let principal = Principal::new(auth_id, user_id, AuthMethod::ApiKey);

    let decision =
        authorizer.is_authorized(&principal, "Action::\"Edit\"", "Document::\"doc-123\"");
    assert_eq!(decision, Decision::Deny);
    assert!(matches!(
        authorizer.authorize(&principal, "Action::\"Edit\"", "Document::\"doc-123\""),
        Err(AuthzError::Forbidden)
    ));

    // Now with Session method -> Allow
    let session_principal = Principal::new(auth_id, user_id, AuthMethod::Session);
    let session_decision = authorizer.is_authorized(
        &session_principal,
        "Action::\"Edit\"",
        "Document::\"doc-123\"",
    );
    assert_eq!(session_decision, Decision::Allow);
    assert!(
        authorizer
            .authorize(
                &session_principal,
                "Action::\"Edit\"",
                "Document::\"doc-123\""
            )
            .is_ok()
    );
}

#[test]
fn test_authorizer_fail_closed_on_invalid_entity_uids() {
    let authorizer = create_test_authorizer();
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);

    // Malformed action string
    let decision = authorizer.is_authorized(
        &principal,
        "InvalidMalformedAction",
        "Document::\"doc-123\"",
    );
    assert_eq!(decision, Decision::Deny);

    // Malformed resource string
    let decision =
        authorizer.is_authorized(&principal, "Action::\"View\"", "InvalidMalformedResource");
    assert_eq!(decision, Decision::Deny);
}

#[test]
fn test_authorizer_fail_closed_on_missing_attribute_evaluation_error() {
    // Policy references resource attribute `is_secret` which is missing from entity data
    let schema_str = r#"
    entity User;
    entity Document {
        is_secret: Bool
    };
    action "View" appliesTo {
        principal: [User],
        resource: [Document]
    };
    "#;
    let policy_str = r#"
    permit(
        principal,
        action == Action::"View",
        resource
    ) when {
        resource.is_secret == false
    };
    "#;
    let (schema, policies) = load_schema_and_policies(schema_str, policy_str).unwrap();
    let authorizer = Authorizer::new(schema, policies);

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);

    // Document is not in Entities, so evaluating resource.is_secret causes evaluation error.
    // Authorizer must fail closed to Deny.
    let decision =
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-123\"");
    assert_eq!(decision, Decision::Deny);

    let auth_res = authorizer.authorize(&principal, "Action::\"View\"", "Document::\"doc-123\"");
    assert!(matches!(auth_res, Err(AuthzError::Forbidden)));
}
