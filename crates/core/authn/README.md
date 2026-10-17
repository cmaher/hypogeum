# hypgm-authn

Core authentication crate for the Hypogeum framework.

`hypgm-authn` provides the foundational identity and authentication contracts across Hypogeum, decoupled from application domain entities and database schemas:
- **Principal & Identity**: `Principal { auth_id, user_id, method }` distinguishing credentials from domain users.
- **Bearer JWT Validation**: Stateless verification of signed JWT access tokens using `jsonwebtoken` (HS256/RS256).
- **Session Integration**: Links `Principal` to `axum-login` and `tower-sessions`, providing a generic `AuthBackend` trait.
- **Multi-Auth Extractors**: Unified Axum extractors `RequireAuth` and `OptionalAuth` inspecting incoming headers and sessions.
- **Redaction Guarantee**: Implements `PublicError` from `hypgm-errors`, ensuring internal validation details and secrets never leak in HTTP responses.

## Core Concepts

### Principal & AuthMethod

```rust
use hypgm_authn::{AuthMethod, Principal};
use uuid::Uuid;

let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);
assert_eq!(principal.auth_id, auth_id);
assert_eq!(principal.user_id, user_id);
assert_eq!(principal.method, AuthMethod::BearerJwt);
```

### Bearer JWT Validation

Validate `Authorization: Bearer <token>` tokens containing `sub` (user_id), `auth_id`, `exp`, and optional issuer/audience claims:

```rust
use hypgm_authn::{JwtConfig, JwtValidator};

// HS256 secret key
let validator = JwtValidator::from_hs256_secret(b"your-secret-key");

// RS256 public key PEM
let validator = JwtValidator::from_rs256_pem(rsa_pem_bytes)?;

// Verify token directly into a Principal
let principal = validator.verify_token("Bearer <token>")?;
```

### Session Integration (axum-login & tower-sessions)

`Principal` implements `axum_login::AuthUser<Uuid>` keyed by `auth_id`. Consuming applications implement `AuthBackend`:

```rust
use async_trait::async_trait;
use hypgm_authn::{AuthBackend, Principal, PrincipalAuthBackend};
use uuid::Uuid;

#[derive(Clone)]
struct MyDbAuthBackend;

#[async_trait]
impl AuthBackend for MyDbAuthBackend {
    async fn resolve_principal(&self, auth_id: Uuid) -> Result<Option<Principal>, hypgm_authn::AuthError> {
        // Query user/credential by auth_id...
        todo!()
    }
}

// Adapt to axum_login::AuthnBackend:
let login_backend = PrincipalAuthBackend::new(MyDbAuthBackend);
```

> **Note**: `PrincipalAuthBackend` is a session-resolution adapter designed to resolve `Principal` instances via `get_user(auth_id)`. Credential verification (such as username/password or OAuth verification) is handled out-of-band by application services prior to calling `auth_session.login(&principal)`.

### Axum Extractors

Endpoints consume `RequireAuth` or `OptionalAuth`:

```rust
use axum::{Extension, Router, routing::get};
use hypgm_authn::{JwtValidator, OptionalAuth, RequireAuth};

async fn protected_handler(RequireAuth(principal): RequireAuth) -> String {
    format!("Authenticated user: {}", principal.user_id)
}

async fn optional_handler(OptionalAuth(principal): OptionalAuth) -> String {
    match principal {
        Some(p) => format!("Hello user {}", p.user_id),
        None => "Hello anonymous".to_string(),
    }
}

let validator = JwtValidator::from_hs256_secret(b"your-secret-key");

let app = Router::new()
    .route("/protected", get(protected_handler))
    .route("/optional", get(optional_handler))
    .layer(Extension(validator));
```
