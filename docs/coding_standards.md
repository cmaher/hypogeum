# Architecture and coding standards

These standards describe reusable engineering practices. Keep project names,
concrete domain types, routes, mount points, crate paths, listener addresses,
environment variable names, and deployment topology in project documentation.

## Architecture and boundaries

Favor stateless components operating on plain data. Dependencies flow from
binary composition roots through transport adapters and services to repositories.
Services can delegate directly to repositories; add managers only when business
behavior needs a distinct component.

- **Objects:** plain data without pools, clients, or workflows.
- **Repository:** a trait accepting and returning plain objects, with an injected
  persistence implementation. Only repositories query application tables.
- **Service:** a trait exposing application behavior independently of transport.
  Implementations validate inputs and coordinate injected dependencies.
- **Manager:** optional business logic component with injected trait dependencies.
- **Transport adapter:** translates HTTP or RPC requests, plain objects, and typed
  errors. Keep handlers thin and business logic in services.
- **Client:** a remote implementation behind a trait. Local and remote services
  should expose the same plain-object contract when they are interchangeable.
- **Binary:** loads configuration, constructs dependencies, checks readiness,
  starts servers, and handles shutdown.

Use constructor injection and keep dependency fields private. Libraries receive
typed configuration, pools, and dependencies explicitly. Separate reusable
infrastructure (`crates/core/` foundation, `crates/ext/` drivers) from domain
behavior (`crates/domain/`), and keep executable composition separate from
reusable libraries.

### Dependency injection and component composition

Use constructor injection (`new(...)`) for all components and services, receiving
all dependencies and configuration explicitly. Avoid mutable builder methods
(`with_*` mutators) or multiple constructor overloads on component structs.

Name dependency bundles `<Name>Components` (e.g. `AppComponents`, `AuthComponents`,
`HelloComponents`). Do not use names like `AppState` or `AuthState` for dependency
injection bundles or composition roots; in Axum, "state" specifically denotes handler
state, whereas component bundles represent composition roots. Group cohesive
configuration parameters into dedicated plain data structs (e.g. `TokenConfig`)
rather than scattering loose scalar parameters across components.

When tests or local development require default dependencies or partial overrides,
use a dedicated factory struct (`<Name>ComponentsFactory`) that implements `Default`
and provides a `.build()` method. Tests can override specific dependencies via
struct update syntax (`..Default::default()`) before calling `.build()`, keeping
production component structs free of test helpers and mutation methods.

## HTTP and RPC composition

Compose HTTP adapters using native Axum routing and nesting. Domain adapters own
local routes; the application composition layer owns global mount points and
shared transport policies. Avoid custom protocol dispatch when upstream routing
already provides the required behavior.

Each domain controller exports a uniform `router(components: <Domain>Components) -> Router<()>`
entry point. Do not create split router entry points (such as `router_with_state`
or `router_with_auth`). Controllers encapsulate their private Axum state internally
and return a self-contained, unparameterized `Router<()>`.

Public exposure must be explicit. Register only intended public services and
keep internal services isolated. Apply CORS deliberately to the intended routes
and origins; do not infer public exposure from registration on another listener.

Define payload limits and document their scope. Extractor limits apply only to
cooperating extractors; a limit on every request requires enforcement at the
request boundary. Test both fixed-length and streamed payload boundaries.

Validate media types before parsing bodies. Use consistent, safe transport error
responses, including routing and serialization failures. Preserve required
protocol headers when translating errors. Define method and trailing-slash
behavior explicitly and test it through the composed router.

Keep health checks independent of infrastructure unless they explicitly represent
readiness. Handle shutdown signals in the binary and drain active requests before
exit. Document TLS termination and ingress protocol requirements separately.

## SQL and migrations

Build typed database queries with SeaORM entities and active models, integrating
with SQLx pool ownership through `SqlxPostgresConnector`. Keep domain migrations
with the domain that owns the schema, using SeaORM migration traits. Raw SQL
remains appropriate for database administration, readiness functions, and upstream
infrastructure APIs. Bind caller-supplied values rather than interpolating them.

Use schema-qualified names and explicit schema ownership. Run migrations through
a dedicated administration entry point with credentials separate from runtime
accounts. Apply upstream migrations before infrastructure contracts and dependent
domain migrations. Provisioning owns roles and initial default privileges.

Never edit an applied migration. Add a new one and update the matching readiness
contract when compatibility changes. Version DDL, functions, and grants in
migrations rather than reconciling them during service startup.

Give runtime accounts only the privileges their services need. Readiness should
use narrow compatibility checks rather than migration-ledger access or runtime
migrations. Return an explicit compatibility version; unrelated migrations must
not change readiness merely by increasing a ledger row count. Verification must
check history and runtime privilege boundaries without applying or repairing
schema changes.

Keep transaction management inside the repository boundary for operations needing
multiple atomic statements. Do not thread SQLx transaction handles through
service APIs. Add a unit-of-work abstraction only when an operation needs one.

## Errors and configuration

Define shared and domain-specific typed errors with `thiserror`, implementing
the `PublicError` and `PublicErrorInfo` contract from `hypgm-errors`. Map them to
transport responses at adapter boundaries (`hypgm-http` renders any `PublicError`
into uniform JSON and attaches classification to response extensions for tracing).
Distinguish validation failures, missing entities, downstream failures, and
persistence failures. Redact internal details in public responses, including
errors encountered while rendering a response. Never expose connection URLs,
credentials, or internal queries. Use `anyhow` for binary-level context.

Only binary composition roots load environment configuration. Load it once,
deserialize typed configuration, and use types that redact credentials when
formatted. Never mutate process environment. Libraries accept explicit values and
injected dependencies.

## Logging and tracing

Initialize logging once at the binary boundary. Support typed logging
configuration and structured output without reading environment variables inside
libraries.

Generate request identifiers at the trust boundary and propagate them through
responses and tracing context. Use matched route templates rather than raw paths
or query strings. Keep shared middleware independent of application route names.
Ensure error events retain request context at supported logging thresholds and
propagate spans explicitly into spawned tasks.

Record request completion status and duration with appropriate severity. Do not
log request bodies, credentials, authorization headers, or sensitive user input.

## Verification

Run the project's formatting, lint, build, and test checks before committing.
Test observable behavior and meaningful failure cases rather than constructors,
private implementation details, or values merely passed through mocks.

Unit tests use injected trait implementations without live infrastructure.
Integration tests exercise real adapters and persistence. Test composed transport
behavior over loopback connections, including failure responses, required headers,
payload limits, request correlation, and graceful shutdown. Exercise exported
middleware helpers as consumers install them.

Database integration suites own their Testcontainers instances and isolate each
test's database. Where SQLx cases run in child processes, pass database
configuration only to the child. Avoid process-global environment mutation,
ambient database URLs, and shared development databases. Match provisioning and
migrations to runtime privilege boundaries. Use synthetic data and temporary
storage.
