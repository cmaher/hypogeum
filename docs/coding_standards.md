# Architecture and coding standards

The workspace favors stateless components operating on plain data. Dependencies
flow from binary composition roots through transport adapters and services to
repositories. Services can delegate directly to repositories; add managers only
when business behavior needs a distinct component.

Reusable infrastructure belongs in `crates/`; domain crates belong in `crates/domain/`.
The generic example is `crates/domain/users/`. See the README's project customization
section before renaming a fresh project and its database schema.

## Boundaries

- **Objects:** data without pools, clients, or workflows. `User` is an example.
- **Repository:** a trait accepting and returning plain objects, with an injected
  PostgreSQL implementation. Only repositories query application tables.
- **Service:** a trait exposing application behavior independently of transport.
  Implementations validate inputs and coordinate injected dependencies.
- **Manager:** optional business logic component with injected trait dependencies.
- **Server:** gRPC adapter translating messages, plain objects and typed errors.
- **Client:** remote implementation behind a trait. The User client implements the
  same plain-object service contract, so callers can inject local or remote services.
- **Binary:** loads config, constructs pools and dependencies, checks readiness,
  starts servers, and handles shutdown.

Use public constructors and keep internal dependency fields private. Unit tests
use trait implementations without live infrastructure; integration tests exercise
real adapters and PostgreSQL through the User repository and transport.

## Public HTTP and RPC composition

`crates/domain/hello-world` is the database-independent public example. Its
`HelloWorldService` accepts plain arguments; its Tonic adapter injects
`Arc<dyn HelloWorldService>` and uses shared typed status mappings. Generated
Tonic clients suffice for this example. No repository is needed.

`crates/web-server` merges Axum health routes with Tonic
`Routes::into_axum_router()` and wraps each explicitly registered public service
in `tonic_web::GrpcWebLayer`. Axum HTTP/2 support is enabled so HTTP/1.1 health,
native HTTP/2 gRPC and binary/text gRPC-Web share a listener. Keep protobuf RPC
semantics and use upstream routing; no custom protocol dispatcher is needed.

Public service registration is an explicit boundary: User CRUD and the task
Board remain on their own listeners. When adding a public service, register its
wrapped adapter in the public Tonic routes before applying route-scoped CORS,
and test its native/browser behavior. CORS allows any origin without credentials
for this unauthenticated example; it applies only to registered public RPC routes,
never ordinary health routes or the unknown-service fallback.

The executable composition root lives in `cmd/web-server/src/main.rs`; the
reusable router stays in `crates/web-server`. Only main loads `EnvSource` once and deserializes `HYPOGEUM_WEB_` configuration
and `HYPOGEUM_` logging. `HYPOGEUM_WEB_BIND` defaults to `127.0.0.1:8080`, using the
existing `HYPOGEUM_ENV_FILE` semantics. Library constructors receive dependencies
explicitly. Shutdown drains active requests on Ctrl-C/SIGTERM. Deployment owns
TLS/ALPN and ingress HTTP/2 forwarding. No frontend or JSON transcoding is needed.

Loopback tests use the same socket for HTTP, native gRPC and both gRPC-Web
encodings, decoding protobuf replies and embedded status trailers. Immediate
errors may use trailers-only status headers; check those too. Text chunks may
be independently base64 padded. Test validation, CORS boundaries, unknown methods,
absence of private services, and graceful shutdown without database infrastructure.

## SQL and migrations

Repositories build typed database queries with SeaORM entities and active models,
integrating with SQLx pool ownership via `SqlxPostgresConnector`. Strongly typed
domain migrations are collocated inside each domain crate (`src/pg/migrations/`) using
SeaORM's `MigratorTrait` and `MigrationTrait`. Raw SQL remains appropriate for
PostgreSQL-specific administration, readiness functions, and upstream infrastructure APIs.
Audit `AssertSqlSafe` boundaries: interpolating caller values into SQL remains forbidden.

Use schema-qualified names. `hypogeum.users` belongs to the User service; `apalis`
belongs to upstream task infrastructure. The explicit migration runner (`hypogeum-db`)
uses a dedicated `hypogeum_migrator` connection (selecting role `hypogeum_owner`) and
orchestrates migrations in order:
1. Upstream Apalis SQLx migrations run first (`apalis._sqlx_migrations`).
2. Infrastructure runtime contracts and privilege hardening apply to `infra` and `apalis`.
3. Domain SeaORM migrators (`users::pg::migrations::Migrator::up`) install domain schemas,
   tables, check constraints, grants, and readiness functions, tracking history under
   domain-qualified tables (e.g. `hypogeum.seaql_migrations`).

Never edit an applied migration. Add a new one and update the matching readiness
contract. Keep DDL, functions and grants versioned within migration definitions rather than
reconciling them from Rust on each service invocation. Provisioning owns roles and initial
default privileges.

`hypogeum_migrator` may select `hypogeum_owner` for DDL. `hypogeum_users`
and `hypogeum_tasks` receive deliberately limited runtime privileges. They
cannot read migration ledgers or each other's data. Readiness uses narrow
version functions instead of ledger access or runtime migrations. These return
an explicit compatibility version; adding an unrelated migration does not change
readiness by changing a ledger row count. The separate `verify` command checks
history and runtime boundaries without applying or repairing schema changes.

For operations needing multiple atomic statements, keep transaction management
inside the repository boundary. Do not thread SQLx transaction handles through
service APIs. Add a unit-of-work abstraction only when an operation needs one.

## Errors and configuration

Shared `thiserror` types live in `crates/errors`. Map validation to InvalidArgument,
missing entities to NotFound and internal/database failures to redacted Internal
responses. `ServiceError::Remote` represents downstream gRPC client failures;
`RepoError::Database` covers PostgreSQL regardless of its location.

Only binary composition roots load environment configuration. Binaries load environment
once via `env-config` (`EnvSource::load`), deserialize a per-binary `Config` with `envy`,
use `Secret` for credentials, and never mutate process environment. Libraries accept
explicit values and injected dependencies. Use `anyhow` for binary-level context.
Do not include connection URLs, credentials or internal queries in public errors.

## Verification

`cargo make ci` checks format, denies Clippy warnings, builds and runs the entire
workspace. Git and Python 3.9 or newer run the rename-script tests; Docker is the only
infrastructure prerequisite.
`cargo test --locked --workspace --all-features` runs every integration suite.

Each suite launcher owns one Testcontainers PostgreSQL instance. It launches its
ignored SQLx cases with a child-only `DATABASE_URL`; `sqlx::test` creates a fresh
database for every case. This avoids process-global environment mutation and
parallel-test interference. Provisioning and migrations match the runtime role
boundaries used by the application. Use synthetic data and temporary storage.

Add tests for observable behavior and meaningful failure cases. Avoid tests that
merely restate constructors or private implementation details.
