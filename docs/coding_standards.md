# Architecture and coding standards

The workspace favors stateless components operating on plain data. Dependencies
flow from binary composition roots through transport adapters and services to
repositories. Services can delegate directly to repositories; add managers only
when business behavior needs a distinct component.

Executable composition roots for the public app server, Apalis dashboard and
`hypogeum-db` CLI belong in `cmd/app`, `cmd/apalis-dashboard` and `cmd/db`.
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
- **Controller:** thin Axum adapter mapping HTTP requests and responses, extracting
  `AppJson`, invoking services, and returning `Result<AppJson<T>, HttpError>`.
- **Server:** gRPC adapter translating messages, plain objects and typed errors for
  internal services.
- **Client:** remote implementation behind a trait. The User client implements the
  same plain-object service contract, so callers can inject local or remote services.
- **Binary:** loads config, constructs pools and dependencies, checks readiness,
  starts servers, and handles shutdown.

Use public constructors and keep internal dependency fields private. Unit tests
use trait implementations without live infrastructure; integration tests exercise
real adapters and PostgreSQL through the User repository and transport.

## Public HTTP and internal RPC composition

`crates/domain/hello-world` is the database-independent public example. Its
`HelloWorldService` accepts plain arguments; its thin Axum controller injects
`Arc<dyn HelloWorldService>`, extracts `AppJson`, and uses shared typed error
mappings via `HttpError`. No repository is needed.

`crates/domain/app` composes public HTTP routes using native Axum nesting:
`GET /healthz` for health checks, and `/api` nesting domain controllers (such as
`/hello` -> controller `POST /`). It applies a strict 16KiB body limit
(`DefaultBodyLimit`), request tracing middleware (`trace_middleware`), and JSON
fallback handlers for 404 and 405. Domains own no global mount prefixes; the app
router owns all mount points.

Internal service boundaries remain isolated: User CRUD (`crates/domain/users`)
retains internal Tonic gRPC on its private listener (`127.0.0.1:50051`), and the
task Board remains on `127.0.0.1:8082`. No CORS permission headers are emitted
by the public app listener by default.

The executable composition root lives in `cmd/app/src/main.rs`. Main loads
`EnvSource` once, initializes structured logging (`crates/logging`), and
deserializes `HYPOGEUM_APP_` and `HYPOGEUM_LOG` configuration. `HYPOGEUM_APP_BIND`
defaults to `127.0.0.1:8080`, using standard `HYPOGEUM_ENV_FILE` semantics.
Shutdown drains active requests on Ctrl-C/SIGTERM. See [docs/app.md](app.md)
for full architectural details.

Loopback tests verify the public HTTP contract: health probes (GET and HEAD),
canonical routing and trailing-slash 404s, JSON error payloads, 16KiB limits on
both fixed-length and chunked transfers, method enforcement with Allow headers,
request ID propagation, structured log timing, and graceful shutdown draining.

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
For HTTP controllers, `crates/http-support` automatically maps these domain
errors via `HttpError` into safe JSON error responses (`validation_error`,
`not_found`, `internal_error`). Database and internal details are never leaked
to HTTP clients.

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
