# hypgmx-kratos

Typed client for the [Ory Kratos](https://www.ory.sh/kratos/) v1 **public** API. It covers only what a
server-rendered app needs for browser sessions. It contains no application policy such as approval,
roles or redirects.

| Method | Kratos endpoint | Cookie sent |
| --- | --- | --- |
| `whoami(&SessionCookie)` | `GET /sessions/whoami` | only `ory_kratos_session=<value>` |
| `get_flow(kind, id, Option<&HeaderValue>)` | `GET /self-service/{login,registration,settings}/flows?id=` | the browser's full `Cookie` header, which Kratos needs for CSRF |
| `get_error(id)` | `GET /self-service/errors?id=` | none |
| `logout_url(&SessionCookie)` | `GET /self-service/logout/browser` | only `ory_kratos_session=<value>` |
| `ready()` | `GET /health/ready` | none |

`session_cookie(&HeaderMap) -> Option<SessionCookie>` is the only way to build a `SessionCookie`. It
takes the first non-empty `ory_kratos_session` cookie across all `Cookie` headers and skips values
outside the RFC 6265 cookie-value alphabet, so no other cookie can be forwarded. Its `Debug` output
hides the token.

```rust
let client = KratosClient::new(KratosConfig {
    public_url: "http://127.0.0.1:4433".parse()?,
    request_timeout: DEFAULT_REQUEST_TIMEOUT,
})?;
if let Some(cookie) = session_cookie(request.headers()) {
    let session = client.whoami(&cookie).await?; // active sessions only
}
```

The base URL and timeout are typed constructor arguments. The library never reads the environment.
Redirects are never followed. The client is HTTP-only because it targets a Kratos on the local network.
`KratosClient::new` returns a `ConfigError` for a non-`http` URL (no TLS backend is compiled in), a
URL with a path (only root URLs are supported), or a zero timeout.

## Errors

`KratosError` implements `hypgm_errors::PublicError`. Its variants carry no Kratos response bodies,
URLs or cookies.

| Variant | Cause | Public status |
| --- | --- | --- |
| `Unauthenticated` | whoami/logout 401; inactive session; settings flow 401 | 401 |
| `FlowGone` | flow or error 400/403/404/410 (unknown, expired, or started in another browser) | 410 |
| `Unavailable` | connect failure, timeout, 5xx, not ready | 500 |
| `Unexpected` | unparseable or incomplete body (e.g. missing `active` or `traits`), redirect, other statuses (including whoami/logout 403) | 500 |

Callers should fail closed on `Unavailable` and `Unexpected`.

## Models

`Session { id, active, identity }`, `Identity { id, traits: serde_json::Value }`,
`Flow { ui: UiContainer { action, method, nodes, messages } }`,
`UiNode { group, attributes, messages, meta.label }`, `UiText { kind, text }` and
`FlowError { error: { message } }`. Input attributes are typed (`UiNodeAttributes::Input`). Other
node kinds parse as `UiNodeAttributes::Other`. Self-service error `reason`/`debug` fields are
deliberately not parsed.
