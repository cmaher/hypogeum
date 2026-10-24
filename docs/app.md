# App Architecture and HTTP API

The `app` binary (`example/cmd/app`) is the public HTTP entry point for the Exemplum reference application. It replaces legacy public RPC with a modular, composed [Axum](https://docs.rs/axum) HTTP service providing JSON APIs, server-rendered HTML pages with progressive HTMX enhancement, structured health monitoring, bounded payload handling, typed error envelopes, and integrated distributed tracing.

Internal services (such as User CRUD in `example/crates/domain/users`) retain internal gRPC over private listeners (`127.0.0.1:50051`), while public endpoints are exposed through `app` (`127.0.0.1:8080`).

---

## 1. Composition Root and Architectural Boundaries

```
example/cmd/app (main.rs)
  │  Loads EXEMPLUM_APP_* config & EXEMPLUM_LOG_* via crates/core/envconfig
  │  Initializes crates/core/logging subscriber
  │  Constructs domain services (Arc<dyn Service>)
  │  Wired to graceful shutdown signal
  ▼
example/crates/app (router.rs)
  │  Composes top-level router: GET /healthz, /api, page routes, /assets
  │  Applies 16KiB DefaultBodyLimit & trace_layer from crates/core/http
  │  Installs JSON 404 fallback for /api and HTML 404 fallback for pages
  ▼
example/crates/domain/hello-world (controller.rs)   example/crates/domain/hello-world (view.rs)
  │  Extracts AppJson<HelloRequest>                │  Extracts AppForm<GreetingForm> + HtmxRequest
  │  Maps HelloWorldError -> HttpError (JSON)       │  Renders Askama page or section (HTML)
  ▼                                                ▼
example/crates/domain/hello-world (service.rs)
     Pure business logic (HelloWorldService trait & DefaultHelloWorldService)
     Returns Result<T, HelloWorldError> implementing PublicError
     Zero HTTP / Axum / transport dependencies
```

### Component Responsibilities

1. **`example/cmd/app` (`exemplum-app-cli` package):**
   The composition root executable.
   - Loads environment configuration via `hypgm-envconfig` (`crates/core/envconfig`) once at startup.
   - Initializes structured logging via `hypgm-logging` (`crates/core/logging`).
   - Constructs dependencies and injects them as trait objects (`Arc<dyn HelloWorldService>`).
   - Binds the TCP listener (default `127.0.0.1:8080`).
   - Handles graceful shutdown on `SIGINT` (Ctrl+C) and `SIGTERM`, draining in-flight requests.

2. **`example/crates/app` (`exemplum-app` package):**
   The application glue router.
   - Mounts health checks (`GET /healthz`), API namespaces (`/api`), domain page routes, and shared view assets (`/assets`).
   - Enforces global HTTP policies: 16KiB raw body limits, request tracing layer, and 404/405 error translations.
   - Accepts injected domain services; contains no business logic.

3. **Domain Layer (`example/crates/domain/<domain>`):**
   - **`src/service.rs`:** Domain service trait and implementation. Operates strictly on plain Rust types and returns `Result<T, DomainError>` where `DomainError` implements `PublicError`. Contains no HTTP concepts or Axum dependencies.
   - **`src/api.rs`:** DTOs for JSON requests and responses, deriving `serde::Deserialize` and `serde::Serialize`.
   - **`src/controller.rs`:** Thin HTTP transport adapter. Defines route handlers that extract `AppJson<Req>`, invoke the domain service, and return `Result<AppJson<Res>, HttpError>`. Returns a self-contained `Router<()>` with its private state injected.
   - **`src/view.rs` and `templates/`:** Optional HTML adapter. Domain-owned Askama templates and handlers built on `hypgm-view` (`crates/core/view`). Views call the same injected services as controllers directly; they never call the JSON API over HTTP. See [HTML Views and HTMX](#7-html-views-and-htmx).

---

## 2. Routing Architecture and Composition

### Composing Routes with Native Axum Nesting

Routers are composed hierarchically using native Axum `.nest()`:

```rust
pub fn router<S>(components: AppComponents<S>) -> Router<()>
where
    S: tower_sessions::SessionStore + Clone + Send + Sync + 'static,
{
    let hello_components = HelloComponents::new(
        components.hello_service,
        components.authorizer,
        components.user_service,
    );
    let hello = hello_world::controller::router(hello_components.clone());
    let hello_pages = hello_world::view::router(hello_components);
    // ... auth routes ...

    // `/api` owns JSON errors for every path beneath it.
    let api = Router::new()
        .nest("/hello", hello)
        .nest("/auth", auth_routes)
        .fallback(not_found_handler);

    // Page routes own HTML errors.
    let pages = Router::new()
        .merge(hello_pages)
        .nest(hypgm_view::assets::ASSETS_MOUNT, hypgm_view::assets::router())
        .layer(axum::middleware::from_fn(html_error_middleware));

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .route("/api/", any(not_found_handler))
        .merge(pages)
        .fallback(html_not_found_handler)
        .layer(auth_layer)
        .layer(Extension(jwt_validator))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(trace_layer())
}
```

- Each domain controller returns a self-contained `Router<()>` accepting its own domain `Components`.
- Mount points are owned by `app/src/router.rs`, ensuring domain crates remain independent of global URL prefix assumptions.
- Canonical endpoints:
  - `GET /healthz` - Health probe.
  - `POST /api/hello` - Composed endpoint from `/api` -> `/hello` -> controller `POST /`.
  - `GET /` - Greeting page from `hello_world::view::router`.
  - `POST /hello` - Greeting form submission (`application/x-www-form-urlencoded`).
  - `GET /assets/htmx-<version>.min.js`, `GET /assets/view.js` - Shared view assets.

### Concrete Request / Response Example

**Request:**
```http
POST /api/hello HTTP/1.1
Host: 127.0.0.1:8080
Content-Type: application/json
Accept: application/json
X-Request-Id: caller-client-id-123

{"name": "Ada"}
```

**Response:**
```http
HTTP/1.1 200 OK
Content-Type: application/json
X-Request-Id: 550e8400-e29b-41d4-a716-446655440000

{"message": "Hello, Ada!"}
```
*(Note: untrusted incoming `X-Request-Id` headers are replaced with a server-generated UUID v4).*

### Steps to Add a New Domain Service

1. Create or extend `crates/domain/<new-domain>/`.
2. Define plain service trait and implementation in `src/service.rs`:
   ```rust
   #[async_trait::async_trait]
   pub trait WidgetService: Send + Sync {
       async fn get_widget(&self, id: &str) -> Result<Widget, WidgetError>;
   }
   ```
   *(where `WidgetError` implements `PublicError`).*
3. Define request/response DTOs in `src/api.rs`.
4. Create a thin Axum controller in `src/controller.rs` taking domain components:
   ```rust
   pub fn router(components: WidgetComponents) -> Router<()> {
       Router::new()
           .route("/{id}", get(get_widget))
           .with_state(components)
   }
   ```
5. In `example/crates/app/src/router.rs`, nest the new controller under `/api`:
   ```rust
   let widget_components = WidgetComponents::new(components.widget_service);
   let widgets = widget::controller::router(widget_components);
   let api = Router::new()
       .nest("/hello", hello)
       .nest("/widgets", widgets);
   ```
6. In `AppComponents`, include the new dependency with constructor injection.

---

## 3. Typed Error Handling and PublicError Contract

Shared error handling in `crates/core/http` (`hypgm-http`) bridges application domain errors with HTTP responses without leaking internal details, powered by the `PublicError` and `PublicErrorInfo` contract defined in `crates/core/errors` (`hypgm-errors`).

### Error Mapping Pipeline

```
Repository Error (RepoError: PublicError)
      │
      ▼
Domain Service Error (e.g. HelloWorldError, UserError: PublicError)
      │
      ▼ ? (Blanket From<E: PublicError> for HttpError)
HTTP Transport Error (HttpError)
      │
      ▼ IntoResponse
JSON Error Envelope (Safe status, code, message) + Extensions (PublicErrorInfo)
```

`hypgm-http` renders any type implementing `PublicError` into the standard JSON error payload and attaches its `PublicErrorInfo` (and status/code) to response extensions for tracing. `HttpError` provides a blanket `From<E: PublicError>` implementation. Inside controllers, `?` automatically converts domain errors implementing `PublicError` into standard HTTP responses:

| Source Error | HTTP Status | Error Code (`code`) | Message Behavior |
| --- | --- | --- | --- |
| `UserError::Validation(_)` | `400 Bad Request` | `"validation_error"` | `"Validation failed"` (details redacted) |
| `RepoError::NotFound` | `404 Not Found` | `"not_found"` | `"Resource not found"` |
| `RepoError::Database(_)` | `500 Internal Server Error` | `"internal_error"` | `"Internal server error"` (details redacted) |
| `UserError::Remote(_)` | `500 Internal Server Error` | `"internal_error"` | `"Internal server error"` (details redacted) |
| Response serialization failure | `500 Internal Server Error` | `"internal_error"` | `"Internal server error"` (details redacted) |
| Malformed JSON body | `400 Bad Request` | `"invalid_request"` | `"Invalid request"` |
| Consumed body exceeds 16KiB | `413 Payload Too Large` | `"payload_too_large"` | `"Payload too large"` |
| Unsupported `Content-Type` | `415 Unsupported Media Type` | `"unsupported_media_type"` | `"Unsupported media type"` |
| Route not found | `404 Not Found` | `"not_found"` | `"Resource not found"` |
| Method not allowed | `405 Method Not Allowed` | `"method_not_allowed"` | `"Method not allowed"` (includes `Allow` header) |

### Uniform Error Response Format

Application error responses adhere to the standard JSON envelope. The OAuth token
endpoint uses `error` and `error_description` fields, supplies a matching challenge
for failed HTTP authentication, and disables response caching:

```json
{
  "code": "validation_error",
  "message": "Validation failed"
}
```

### Contextual Reclassification

If a controller needs to map a domain error to a specific classification (for example, interpreting a missing entity as an invalid request parameter), use `.reclassify()`:

```rust
let item = service.find(id).await.map_err(|e| {
    HttpError::from(e).reclassify(PublicErrorInfo::INVALID_REQUEST)
})?;
```

---

## 4. HTTP Transport Policies and Hardening

- **16KiB Raw Body Limit (Extractor-Scoped):**
  Request payloads consumed by the application are constrained to `MAX_BODY_BYTES = 16_384` bytes. Enforced by Axum's `DefaultBodyLimit` for cooperating extractors and strictly validated by the `AppJson` extractor for both fixed-length and chunked/streamed transfers. Exceeding payloads return `413 Payload Too Large`. Unread request bodies on endpoints that do not consume payloads (such as `GET /healthz` or unmapped routes) are ignored and not buffered by this layer; future extractors must explicitly consume bounded bodies.
- **Strict Content-Type Validation:**
  `AppJson` requires valid MIME syntax with `Content-Type: application/json` or structured JSON subtypes (`application/<subtype>+json`). Malformed media types, syntax errors, or non-JSON media types return `415 Unsupported Media Type`.
- **Trailing Slash Discipline:**
  `/api/hello` is the canonical route. Requesting `/api/hello/` returns `404 Not Found` without redirect, preventing ambiguous redirection loops. Page paths have no trailing slash except `/`; `/hello/` returns an HTML `404 Not Found`.
- **Error Format by Namespace:**
  Every path under `/api` (including `/api` and `/api/`) returns the JSON error envelope. Page routes, `/assets`, and unknown paths outside `/api` return safe HTML errors that carry the same classification, status, `Allow` header, and `X-Request-Id`. `/healthz` behavior is unchanged.
- **Strict Method Enforcement:**
  Unsupported methods on registered routes return `405 Method Not Allowed` along with a valid `Allow` header (e.g., `Allow: POST`).
- **Same-Origin Defaults:**
  No permissive CORS headers (`Access-Control-Allow-Origin: *`) are attached by default.
- **No Implicit OPTIONS:**
  `OPTIONS` requests to endpoints that do not define an OPTIONS handler return `405 Method Not Allowed`.
- **Health Probing:**
  `GET /healthz` returns `200 OK` with `text/plain; charset=utf-8` body `ok`. `HEAD /healthz` returns `200 OK` with empty body and matching headers. Neither probe touches the database.

---

## 5. Structured Logging and Distributed Tracing

Logging and request tracing are provided by `crates/core/logging` (`hypgm-logging`) and `crates/core/http` (`hypgm-http`).

### Subscriber Setup (`crates/core/logging`)

The subscriber is initialized once in `example/cmd/app/src/main.rs`:

```rust
logging::init(&config.log, config.log_format)?;
```

- **Filter string (`HYPOGEUM_LOG`):** Standard `tracing-subscriber` env-filter syntax (default `info`).
- **Format (`HYPOGEUM_LOG_FORMAT`):**
  - `text`: ANSI-colorized, human-readable terminal output.
  - `json`: Machine-readable newline-delimited JSON for log aggregators.

### Request Tracing Layer (`crates/core/http`)

Every HTTP request traverses `trace_layer` (`TraceLayer`):

1. **Request ID Generation:**
   A UUID v4 is generated for every request. Any untrusted `X-Request-Id` supplied by the client is rejected and replaced. The generated ID is injected into:
   - The response headers (`X-Request-Id`).
   - Request extensions (`RequestId`).
   - The tracing span (`request_id`).
2. **Span Context:**
   Creates a request span with fields:
   `request{request_id, method, matched_path, status, elapsed_ms, error_code}`
3. **Completion Event:**
   Exactly one completion event is emitted when the response is ready:
   - `DEBUG` for successful health responses marked with `Extension(HealthCheck)` (`/healthz` in this app). The shared middleware does not depend on application route names.
   - `ERROR` for `5xx` internal server errors.
   - `INFO` for all other requests.
4. **Data Redaction:**
   Query strings, request bodies, user names, passwords, and sensitive authorization headers are never logged.
5. **Async Context Inheritance in Spawned Tasks:**
   Background tasks spawned during request processing should inherit span context using `.instrument(Span::current())`:
   ```rust
   let span = tracing::Span::current();
   tokio::spawn(async move {
       // Logs emitted here inherit the request_id
   }.instrument(span));
   ```

---

## 6. Running and Configuration

### Configuration Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `EXEMPLUM_APP_BIND` | `127.0.0.1:8080` | Socket address for public HTTP listener |
| `EXEMPLUM_APP_DATABASE_URL` | required | PostgreSQL URL using restricted `exemplum_users` credentials |
| `EXEMPLUM_APP_JWT_SECRET` | required | Private signing secret, at least 32 bytes |
| `EXEMPLUM_APP_MAX_PASSWORD_VERIFICATIONS` | `4` | Maximum concurrent password checks; must be positive and fit the semaphore capacity |
| `EXEMPLUM_APP_SESSION_CLEANUP_INTERVAL_SECONDS` | `3600` | Positive interval between expired-session cleanup runs |
| `EXEMPLUM_APP_CEDAR_SCHEMA` | `config/schema.cedarschema` | Schema loaded at startup |
| `EXEMPLUM_APP_CEDAR_POLICIES` | `config/policies.cedar` | Policies loaded at startup |
| `EXEMPLUM_LOG` | `info` | Tracing filter directive |
| `EXEMPLUM_LOG_FORMAT` | `text` | Log format: `text` or `json` |
| `EXEMPLUM_ENV_FILE` | *(unset)* | Selects `.env` file (e.g. `env/local.env`; empty disables) |

### Local Execution

Follow [the example setup](../example/README.md#running-the-application) to
provision and migrate PostgreSQL and configure a private signing secret. The
binary checks the runtime adapters before opening its listener and never
migrates on startup. Run from `example/` so the default policy paths resolve:

```sh
cargo make app
```

Or run the binary directly:

```sh
cargo run --locked -p exemplum-app-cli --bin app
```

Test with curl:

```sh
# Health check
curl -i http://127.0.0.1:8080/healthz

# JSON API
curl -i -X POST http://127.0.0.1:8080/api/hello \
  -H "Content-Type: application/json" \
  -d '{"name": "World"}'
```

---

## 7. HTML Views and HTMX

Server-rendered pages work as ordinary HTML forms and are progressively enhanced
with [HTMX](https://htmx.org/). Reusable support lives in `hypgm-view`
(`crates/core/view`); greeting routes and templates live in
`example/crates/domain/hello-world` (`src/view.rs`, `templates/`).

### Routes

| Route | Behavior |
| --- | --- |
| `GET /` | Full greeting document. |
| `POST /hello` | Consumes `application/x-www-form-urlencoded`. `HX-Request: true` returns the greeting section; ordinary submissions return the full document. |
| `GET /assets/htmx-2.0.11.min.js` | Vendored, pinned HTMX release (`crates/core/view/assets/LICENSE.htmx` records version, checksum and license). |
| `GET /assets/view.js` | Shared HTMX configuration. |

Greeting is non-mutating, so `POST /hello` renders directly. Future
state-changing forms should use post/redirect/get. `Content-Type` controls body
parsing only; there is no content-type dispatch between HTML and JSON adapters.

### `hypgm-view` building blocks

- **`HtmxRequest`:** Extracts `ViewMode::Fragment` only for an exact
  `HX-Request: true`. History restoration requests (`HX-History-Restore-Request: true`)
  always receive full pages.
- **HTML responses:** `render_template`, `render_error_template` and
  `html_response` set `text/html; charset=utf-8`, append `Vary: HX-Request` and
  `Vary: HX-History-Restore-Request` without replacing existing `Vary` values,
  and add `X-Content-Type-Options: nosniff` and `X-Frame-Options: DENY` unless
  already set. A template rendering failure logs its cause and template type
  server-side and returns a minimal, template-free HTML 500
  (`render_failure_response`). Handlers that render a template themselves, for
  example to post-process the body as the greeting view does, call
  `render_failure_response` on failure and `with_classification` to mark error
  responses.
  `render_template` is for non-error statuses; render error pages with
  `render_error_template` so routing middleware does not replace them with
  generic 404/405 pages (debug builds assert this).
- **`AppForm<T>`:** Bounded URL-encoded form extractor using `MAX_BODY_BYTES`
  for both declared and streamed bodies. Rejections are safe HTML `415`, `413`
  and `400` responses.
- **Cross-site forms:** URL-encoded posts need no CORS preflight, so `AppForm`
  rejects browser submissions from other sites with an HTML `403` before
  reading the body: `Sec-Fetch-Site` must be `same-origin` or `none`, or,
  without it, `Origin` must match the request's `Host`. Requests without
  either header (non-browser clients) are accepted. Page routers also install
  `cross_site_middleware`, which applies the same check to every unsafe
  method (anything but `GET`, `HEAD`, `OPTIONS` and `TRACE`) before routing,
  so handlers without a form body are covered. Session cookies also
  default to `SameSite=Strict` (`tower-sessions`); keep that default for
  form-authenticated pages. Deployments behind a proxy that rewrites `Host`
  must forward the original host.
- **HTML errors:** `HtmlError`, `html_error_response` and `html_error_middleware`
  reuse `PublicErrorInfo` classification and attach the same tracing extensions
  as JSON errors. Askama autoescapes all interpolated values; templates never
  mark user input as safe.
- **Assets:** `hypgm_view::assets::router()` serves only the listed embedded
  files. There is no CDN, frontend build or filesystem serving.

### Form section convention

A domain wraps each enhanced form and its results in an element marked
`data-view-section`. The form keeps its ordinary `method` and `action` and adds
`hx-post` to the same URL with `hx-target="closest [data-view-section]"` and
`hx-swap="outerHTML"`. The section contains an empty `data-view-feedback`
element. Fragment responses render the same section markup that the full page
includes.

`view.js` sets `historyRestoreAsHxRequest = false`, swaps HTML `4xx`/`5xx`
responses containing a `data-view-section` (for example validation errors with
status `400`) without rewriting their status, and shows the safe message from
shared HTML errors or a generic message for non-HTML responses, network
failures and timeouts in the section's feedback element. `hx-boost` and history
pushing are not enabled.

### Browser smoke checks

`example/tests/browser` contains a Playwright suite that runs with JavaScript
enabled and disabled against `example/crates/app/examples/view_smoke_server.rs`,
which composes the real router with in-memory services and no database. It is
the behavioural coverage for `view.js` and requires Node.js:

```sh
cargo make --cwd example test-browser
```

The submitted name `server-error` makes the smoke server's injected service fail,
exercising server-error feedback; network failures are simulated by the browser.
