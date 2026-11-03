# hypgm-authn

Authentication contracts shared by Hypogeum applications. The crate does not
verify credentials or sessions itself: an application middleware does that
(Exemplum's `session_gate` asks Ory Kratos through `hypgmx-kratos`) and inserts
a `Principal` into request extensions. Handlers only read it from there.

- **`Principal { session_id, user_id }`**: the verified session and the
  application user it belongs to.
- **`RequireAuth`**: extracts the `Principal` or rejects with a uniform JSON
  `401` (`{"code":"unauthenticated","message":"Authentication required"}`),
  with the `PublicErrorInfo` attached to response extensions for tracing.
- **`OptionalAuth`**: `Some(principal)` or `None` for anonymous requests; never
  rejects.
- **`AuthError`**: implements `PublicError` from `hypgm-errors`.

There is no password hashing, session store, token or API-key support; those
belong to the identity service.

```rust
use axum::{Router, routing::get};
use hypgm_authn::{OptionalAuth, RequireAuth};

async fn protected(RequireAuth(principal): RequireAuth) -> String {
    format!("Authenticated user: {}", principal.user_id)
}

async fn optional(OptionalAuth(principal): OptionalAuth) -> String {
    match principal {
        Some(p) => format!("Hello user {}", p.user_id),
        None => "Hello anonymous".to_string(),
    }
}

// An authentication middleware layered on this router inserts the Principal.
let app: Router = Router::new()
    .route("/protected", get(protected))
    .route("/optional", get(optional));
```

A middleware that cannot decide (for example, the identity service is down)
must fail the request rather than leave the `Principal` out, or the request is
treated as anonymous.
