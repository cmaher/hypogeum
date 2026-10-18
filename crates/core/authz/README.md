# hypgm-authz

Core authorization crate for the Hypogeum framework.

`hypgm-authz` provides policy-based access control powered by AWS Cedar (`cedar-policy`), operating on generic entities (principals, actions, resources, context) without dependencies on application domains or database schemas:
- **Cedar Policy Engine**: In-memory `Authorizer` wrapping `cedar_policy::PolicySet` and `cedar_policy::Schema` with strict fail-closed evaluation.
- **Declarative Loader**: Validates `.cedarschema` schemas and `.cedar` policies from strings or disk files at startup.
- **Principal & Context Mapping**: Maps `Principal` from `hypgm-authn` to Cedar `User::<user_id>` with execution context attributes (`auth_id`, `auth_method`).
- **Axum Integration**: `RequirePermission` extractor, `require_permission`, and `require_permission_with_entities` guard helpers, returning uniform HTTP 401 Unauthorized for unauthenticated callers and HTTP 403 Forbidden (`PublicError`) on policy denial.
- **Diagnostics Redaction**: Internal policy diagnostics and evaluation errors are redacted from client responses and logged at `warn` level.

## Core Concepts

### Policy & Schema Loading

```rust
use hypgm_authz::load_schema_and_policies;

let schema_src = r#"
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
"#;

let policy_src = r#"
permit(
    principal,
    action == Action::"View",
    resource
) when {
    context.auth_method == "bearer_jwt"
};
"#;

let (schema, policies) = load_schema_and_policies(schema_src, policy_src)?;
```

### In-Memory Authorizer

```rust
use hypgm_authz::{Authorizer, Decision};
use hypgm_authn::{AuthMethod, Principal};
use uuid::Uuid;

let authorizer = Authorizer::new(schema, policies)?;

let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4(), AuthMethod::BearerJwt);
let decision = authorizer.is_authorized(&principal, "Action::\"View\"", "Document::\"doc-123\"");
assert_eq!(decision, Decision::Allow);

// Or using `authorize` to return Result<(), AuthzError>:
authorizer.authorize(&principal, "Action::\"View\"", "Document::\"doc-123\"")?;
```

### Axum Guard & Extractors

Enforce permissions in Axum route handlers using declarative policy extractors or programmatic helpers:

```rust
use std::sync::Arc;
use axum::{Router, Json, routing::get, extract::State};
use hypgm_authn::{Principal, RequireAuth};
use hypgm_authz::{
    Authorizer, AuthzRejection, PermissionPolicy, RequirePermission, require_permission,
    require_permission_with_entities,
};

struct ViewDocumentPolicy;
impl PermissionPolicy for ViewDocumentPolicy {
    fn action() -> &'static str { "Action::\"View\"" }
    fn resource() -> &'static str { "Document::\"public\"" }
}

// Extractor guard (authenticates caller and verifies policy)
async fn static_guard_handler(
    _perm: RequirePermission<ViewDocumentPolicy>,
) -> &'static str {
    "Authorized"
}

// Composed with RequireAuth extractor
async fn composed_handler(
    RequireAuth(principal): RequireAuth,
    _perm: RequirePermission<ViewDocumentPolicy>,
) -> String {
    format!("Authorized user {}", principal.user_id)
}

// Programmatic helper inside handler with dynamic resource
async fn dynamic_handler(
    State(authorizer): State<Arc<Authorizer>>,
    RequireAuth(principal): RequireAuth,
) -> Result<Json<&'static str>, AuthzRejection> {
    require_permission(&authorizer, &principal, "Action::\"View\"", "Document::\"dynamic\"")?;
    Ok(Json("Authorized"))
}

let app = Router::new()
    .route("/static", get(static_guard_handler))
    .route("/composed", get(composed_handler))
    .route("/dynamic", get(dynamic_handler))
    .with_state(Arc::new(authorizer));
```

Authorizer constructors and entity replacement return `Result` so invalid entity graphs
fail during configuration. Schema action entities are always retained; runtime actions
must conform to the schema and cannot replace its action hierarchy.
