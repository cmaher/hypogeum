# App Architecture and HTTP API

The `app` binary (`cmd/app`) is the public HTTP entry point for the Hypogeum workspace. It replaces legacy public RPC with a modular, composed [Axum](https://docs.rs/axum) HTTP service providing JSON APIs, structured health monitoring, bounded payload handling, typed error envelopes, and integrated distributed tracing.

Internal services (such as User CRUD in `crates/domain/users`) retain internal gRPC over private listeners (`127.0.0.1:50051`), while public endpoints are exposed through `app` (`127.0.0.1:8080`).

---

## 1. Composition Root and Architectural Boundaries

```
cmd/app (main.rs)
  │  Loads HYPOGEUM_APP_* config & HYPOGEUM_LOG_*
  │  Initializes crates/logging subscriber
  │  Constructs domain services (Arc<dyn Service>)
  │  Wired to graceful shutdown signal
  ▼
crates/domain/app (router.rs)
  │  Composes top-level router: GET /healthz, /api
  │  Applies 16KiB DefaultBodyLimit & trace_middleware
  │  Installs JSON 404 fallback
  ▼
crates/domain/hello-world (controller.rs)
  │  Extracts AppJson<HelloRequest> (16KiB limit, JSON validation)
  │  Maps ServiceError -> HttpError
  │  Returns AppJson<HelloResponse>
  ▼
crates/domain/hello-world (service.rs)
     Pure business logic (HelloWorldService trait & DefaultHelloWorldService)
     Zero HTTP / Axum / transport dependencies
```

### Component Responsibilities

1. **`cmd/app` (`app-cli` package):**
   The composition root executable.
   - Loads environment configuration via `env-config` once at startup.
   - Initializes structured logging via `crates/logging`.
   - Constructs dependencies and injects them as trait objects (`Arc<dyn HelloWorldService>`).
   - Binds the TCP listener (default `127.0.0.1:8080`).
   - Handles graceful shutdown on `SIGINT` (Ctrl+C) and `SIGTERM`, draining in-flight requests.

2. **`crates/domain/app` (`app` package):**
   The application glue router.
   - Mounts health checks (`GET /healthz`) and API namespaces (`/api`).
   - Enforces global HTTP policies: 16KiB raw body limits, request tracing middleware, and 404/405 error translations.
   - Accepts injected domain services; contains no business logic.

3. **Domain Layer (`crates/domain/<domain>`):**
   - **`src/service.rs`:** Domain service trait and implementation. Operates strictly on plain Rust types and returns `Result<T, ServiceError>`. Contains no HTTP concepts or Axum dependencies.
   - **`src/api.rs`:** DTOs for JSON requests and responses, deriving `serde::Deserialize` and `serde::Serialize`.
   - **`src/controller.rs`:** Thin HTTP transport adapter. Defines route handlers that extract `AppJson<Req>`, invoke the domain service, and return `Result<AppJson<Res>, HttpError>`. Returns a self-contained `Router<()>` with its private state injected.
   - **Future View Boundary:** Server-rendered HTML or HTMX templates live alongside API controllers as separate route handlers within the domain, reusing the same domain services without architectural changes.

---

## 2. Routing Architecture and Composition

### Composing Routes with Native Axum Nesting

Routers are composed hierarchically using native Axum `.nest()`:

```rust
pub fn router(hello_service: Arc<dyn HelloWorldService>) -> Router<()> {
    let hello = hello_world::controller::router(hello_service);
    let api = Router::new().nest("/hello", hello);

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback(not_found_handler)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(from_fn(trace_middleware))
}
```

- Each domain controller returns `Router<()>` with its own dependencies injected via `.with_state(...)`.
- Mount points are owned by `app/src/router.rs`, ensuring domain crates remain independent of global URL prefix assumptions.
- Canonical endpoints:
  - `GET /healthz` - Health probe.
  - `POST /api/hello` - Composed endpoint from `/api` -> `/hello` -> controller `POST /`.

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
       async fn get_widget(&self, id: &str) -> Result<Widget, ServiceError>;
   }
   ```
3. Define request/response DTOs in `src/api.rs`.
4. Create a thin Axum controller in `src/controller.rs`:
   ```rust
   pub fn router(service: Arc<dyn WidgetService>) -> Router<()> {
       Router::new()
           .route("/{id}", get(get_widget))
           .with_state(service)
   }
   ```
5. In `crates/domain/app/src/router.rs`, nest the new controller under `/api`:
   ```rust
   let widgets = widget::controller::router(widget_service);
   let api = Router::new()
       .nest("/hello", hello)
       .nest("/widgets", widgets);
   ```
6. In `cmd/app/src/main.rs`, construct the service and pass it to `router(...)`.

---

## 3. Typed Error Handling

Shared error handling in `crates/http-support` bridges application domain errors with HTTP responses without leaking internal details.

### Error Mapping Pipeline

```
Repository Error (RepoError)
      │
      ▼ ?
Domain Service Error (ServiceError)
      │
      ▼ ?
HTTP Transport Error (HttpError)
      │
      ▼ IntoResponse
JSON Error Envelope (Safe status, code, message)
```

`HttpError` implements `From<ServiceError>` and `From<RepoError>`. Inside controllers, `?` automatically converts service errors into standard HTTP responses:

| Source Error | HTTP Status | Error Code (`code`) | Message Behavior |
| --- | --- | --- | --- |
| `ServiceError::Validation(msg)` | `400 Bad Request` | `"validation_error"` | Safe message passed to client |
| `RepoError::NotFound` | `404 Not Found` | `"not_found"` | `"Resource not found"` |
| `RepoError::Database(_)` | `500 Internal Server Error` | `"internal_error"` | Generic message; details redacted |
| `ServiceError::Remote(_)` | `500 Internal Server Error` | `"internal_error"` | Generic message; details redacted |
| Malformed JSON body | `400 Bad Request` | `"invalid_request"` | `"Invalid request payload"` |
| Body exceeds 16KiB | `413 Payload Too Large` | `"payload_too_large"` | `"Payload too large"` |
| Unsupported `Content-Type` | `415 Unsupported Media Type` | `"unsupported_media_type"` | `"Unsupported media type"` |
| Route not found | `404 Not Found` | `"not_found"` | `"Not found"` |
| Method not allowed | `405 Method Not Allowed` | `"method_not_allowed"` | `"Method not allowed"` (includes `Allow` header) |

### Uniform Error Response Format

All error responses adhere to the standard JSON envelope:

```json
{
  "code": "validation_error",
  "message": "Name may contain at most 200 Unicode scalar values."
}
```

### Contextual Reclassification

If a controller needs to map a domain error to a specific classification (for example, interpreting a missing entity as an invalid request parameter), use `.reclassify()`:

```rust
let item = service.find(id).await.map_err(|e| {
    HttpError::from(e).reclassify(ErrorClassification::INVALID_REQUEST)
})?;
```

---

## 4. HTTP Transport Policies and Hardening

- **16KiB Raw Body Limit:**
  Every request body is constrained to `MAX_BODY_BYTES = 16_384` bytes. Enforced both by Axum's `DefaultBodyLimit` and by the `AppJson` extractor for both fixed-length and chunked/streamed transfers. Exceeding requests are rejected immediately with `413 Payload Too Large`.
- **Strict Content-Type Validation:**
  `AppJson` requires `Content-Type: application/json` or `application/*+json`. Other media types (or missing headers) return `415 Unsupported Media Type`.
- **Trailing Slash Discipline:**
  `/api/hello` is the canonical route. Requesting `/api/hello/` returns `404 Not Found` without redirect, preventing ambiguous redirection loops.
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

Logging and request tracing are provided by `crates/logging` and `crates/http-support`.

### Subscriber Setup (`crates/logging`)

The subscriber is initialized once in `cmd/app/src/main.rs`:

```rust
logging::init(&config.log, config.log_format)?;
```

- **Filter string (`HYPOGEUM_LOG`):** Standard `tracing-subscriber` env-filter syntax (default `info`).
- **Format (`HYPOGEUM_LOG_FORMAT`):**
  - `text`: ANSI-colorized, human-readable terminal output.
  - `json`: Machine-readable newline-delimited JSON for log aggregators.

### Request Tracing Middleware (`crates/http-support`)

Every HTTP request traverses `trace_middleware`:

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
   - `DEBUG` for successful `/healthz` checks (reducing log noise).
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
| `HYPOGEUM_APP_BIND` | `127.0.0.1:8080` | Socket address for public HTTP listener |
| `HYPOGEUM_LOG` | `info` | Tracing filter directive |
| `HYPOGEUM_LOG_FORMAT` | `text` | Log format: `text` or `json` |
| `HYPOGEUM_ENV_FILE` | *(unset)* | Selects `.env` file (e.g. `env/local.env`; empty disables) |

### Local Execution

Start the app locally with Cargo Make:

```sh
cargo make app
```

Or run the binary directly:

```sh
cargo run --locked -p app-cli --bin app
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
