use hypgm_authn::{AuthMethod, Principal};
use hypgm_authz::{Authorizer, AuthzError, Decision, Entities, load_schema_and_policies};
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
        resource: [Document],
        context: {
            auth_id: String,
            auth_method: String
        }
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

#[test]
fn test_authorizer_with_preloaded_entities() {
    let schema_str = r#"
    entity User;
    entity Document {
        is_secret: Bool
    };
    action "View" appliesTo {
        principal: [User],
        resource: [Document],
        context: {
            auth_id: String,
            auth_method: String
        }
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

    let entities_json = serde_json::json!([
        {
            "uid": { "type": "Document", "id": "doc-public" },
            "attrs": { "is_secret": false },
            "parents": []
        },
        {
            "uid": { "type": "Document", "id": "doc-secret" },
            "attrs": { "is_secret": true },
            "parents": []
        }
    ]);
    let entities = Entities::from_json_value(entities_json, Some(&schema)).unwrap();
    let authorizer = Authorizer::with_entities(schema, policies, entities);

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);

    // doc-public has is_secret == false -> Allow
    assert_eq!(
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-public\""),
        Decision::Allow
    );
    assert!(
        authorizer
            .authorize(&principal, "Action::\"View\"", "Document::\"doc-public\"")
            .is_ok()
    );

    // doc-secret has is_secret == true -> Deny
    assert_eq!(
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-secret\""),
        Decision::Deny
    );
    assert!(matches!(
        authorizer.authorize(&principal, "Action::\"View\"", "Document::\"doc-secret\""),
        Err(AuthzError::Forbidden)
    ));
}

#[test]
fn test_authorizer_with_runtime_entities() {
    let schema_str = r#"
    entity User;
    entity Document {
        is_secret: Bool
    };
    action "View" appliesTo {
        principal: [User],
        resource: [Document],
        context: {
            auth_id: String,
            auth_method: String
        }
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
    let authorizer = Authorizer::new(schema.clone(), policies);

    let entities_json = serde_json::json!([
        {
            "uid": { "type": "Document", "id": "dynamic-doc" },
            "attrs": { "is_secret": false },
            "parents": []
        }
    ]);
    let runtime_entities = Entities::from_json_value(entities_json, Some(&schema)).unwrap();

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);

    // Evaluating without entities fails closed
    assert_eq!(
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"dynamic-doc\""),
        Decision::Deny
    );

    // Evaluating with runtime entities permits access
    assert_eq!(
        authorizer.is_authorized_with_entities(
            &principal,
            "Action::\"View\"",
            "Document::\"dynamic-doc\"",
            &runtime_entities
        ),
        Decision::Allow
    );
    assert!(
        authorizer
            .authorize_with_entities(
                &principal,
                "Action::\"View\"",
                "Document::\"dynamic-doc\"",
                &runtime_entities
            )
            .is_ok()
    );
}

#[test]
fn test_authorizer_schema_without_context_permits_access() {
    let schema_str = r#"
    entity User;
    entity Document;
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
    );
    "#;
    let (schema, policies) = load_schema_and_policies(schema_str, policy_str).unwrap();
    let authorizer = Authorizer::new(schema, policies);
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    let decision =
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-123\"");
    assert_eq!(decision, Decision::Allow);
    assert!(
        authorizer
            .authorize(&principal, "Action::\"View\"", "Document::\"doc-123\"")
            .is_ok()
    );
}

#[test]
fn test_authorizer_combines_preloaded_and_runtime_entities() {
    let schema_str = r#"
    entity Group;
    entity User in [Group];
    entity Document {
        owner: User
    };
    action "View" appliesTo {
        principal: [User],
        resource: [Document]
    };
    "#;
    let policy_str = r#"
    permit(
        principal in Group::"Admins",
        action == Action::"View",
        resource
    ) when {
        resource.owner == principal
    };
    "#;
    let (schema, policies) = load_schema_and_policies(schema_str, policy_str).unwrap();

    let user_id = Uuid::new_v4();
    let principal = Principal::new(Uuid::new_v4(), user_id, AuthMethod::BearerJwt);

    // Preloaded entities contain Group hierarchy
    let base_entities_json = serde_json::json!([
        {
            "uid": { "type": "Group", "id": "Admins" },
            "attrs": {},
            "parents": []
        },
        {
            "uid": { "type": "User", "id": user_id.to_string() },
            "attrs": {},
            "parents": [
                { "type": "Group", "id": "Admins" }
            ]
        }
    ]);
    let base_entities = Entities::from_json_value(base_entities_json, Some(&schema)).unwrap();
    let authorizer = Authorizer::with_entities(schema.clone(), policies, base_entities);

    // Runtime entities contain Document resource
    let runtime_entities_json = serde_json::json!([
        {
            "uid": { "type": "Document", "id": "doc-combined" },
            "attrs": {
                "owner": { "type": "User", "id": user_id.to_string() }
            },
            "parents": []
        }
    ]);
    let runtime_entities = Entities::from_json_value(runtime_entities_json, Some(&schema)).unwrap();

    // Without runtime entities, resource attributes are missing -> Deny
    assert_eq!(
        authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-combined\""),
        Decision::Deny
    );

    // Combining preloaded entities (Group membership) with runtime entities (Document owner) -> Allow
    assert_eq!(
        authorizer.is_authorized_with_entities(
            &principal,
            "Action::\"View\"",
            "Document::\"doc-combined\"",
            &runtime_entities
        ),
        Decision::Allow
    );
    assert!(
        authorizer
            .authorize_with_entities(
                &principal,
                "Action::\"View\"",
                "Document::\"doc-combined\"",
                &runtime_entities
            )
            .is_ok()
    );
}

#[test]
fn test_authorizer_with_namespaced_principal_type() {
    let schema_str = r#"
    namespace App {
        entity User;
        entity Document;
        action "View" appliesTo {
            principal: [User],
            resource: [Document]
        };
    }
    "#;
    let policy_str = r#"
    permit(
        principal,
        action == App::Action::"View",
        resource
    );
    "#;
    let (schema, policies) = load_schema_and_policies(schema_str, policy_str).unwrap();
    let authorizer = Authorizer::new(schema, policies)
        .with_principal_type("App::User")
        .expect("App::User is valid");

    assert_eq!(authorizer.principal_type().to_string(), "App::User");

    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);
    assert_eq!(
        authorizer.is_authorized(
            &principal,
            "App::Action::\"View\"",
            "App::Document::\"doc-1\""
        ),
        Decision::Allow
    );
}

#[test]
fn test_authorizer_with_custom_context() {
    let schema_str = r#"
    entity User;
    entity Document;
    action "View" appliesTo {
        principal: [User],
        resource: [Document],
        context: {
            auth_id: String,
            auth_method: String,
            mfa_verified: Bool
        }
    };
    "#;
    let policy_str = r#"
    permit(
        principal,
        action == Action::"View",
        resource
    ) when {
        context.mfa_verified == true
    };
    "#;
    let (schema, policies) = load_schema_and_policies(schema_str, policy_str).unwrap();
    let authorizer = Authorizer::new(schema, policies);

    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);
    let mfa_context =
        cedar_policy::Context::from_json_value(serde_json::json!({ "mfa_verified": true }), None)
            .unwrap();

    assert_eq!(
        authorizer.is_authorized_with_context(
            &principal,
            "Action::\"View\"",
            "Document::\"doc-1\"",
            &mfa_context
        ),
        Decision::Allow
    );
    assert!(
        authorizer
            .authorize_with_context(
                &principal,
                "Action::\"View\"",
                "Document::\"doc-1\"",
                &mfa_context
            )
            .is_ok()
    );

    let non_mfa_context =
        cedar_policy::Context::from_json_value(serde_json::json!({ "mfa_verified": false }), None)
            .unwrap();
    assert_eq!(
        authorizer.is_authorized_with_context(
            &principal,
            "Action::\"View\"",
            "Document::\"doc-1\"",
            &non_mfa_context
        ),
        Decision::Deny
    );
}

#[test]
fn test_authorizer_tuple_as_entity_uid() {
    let authorizer = create_test_authorizer();
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);

    let dec = authorizer.is_authorized(&principal, ("Action", "View"), ("Document", "doc-123"));
    assert_eq!(dec, Decision::Allow);
}
