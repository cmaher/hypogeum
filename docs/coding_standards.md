# Architecture and coding standards

The workspace favors stateless components operating on plain data. Dependencies
flow from binary composition roots through transport adapters and services to
repositories. Services can delegate directly to repositories; add managers only
when business behavior needs a distinct component.

Reusable infrastructure belongs in `crates/`; domain crates belong in `crates/domain/`.
The generic example is `crates/domain/users/`. See the README's schema customization
section before renaming database schemas in a new project.

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

## SQL and migrations

Repositories build typed database queries with SeaORM entities and active models,
integrating with SQLx pool ownership via `SqlxPostgresConnector`. Strongly typed
domain migrations are collocated inside each domain crate (`src/pg/migrations/`) using
SeaORM's `MigratorTrait` and `MigrationTrait`. Raw SQL remains appropriate for
PostgreSQL-specific administration, readiness functions, and upstream infrastructure APIs.
Audit `AssertSqlSafe` boundaries: interpolating caller values into SQL remains forbidden.

Use schema-qualified names. `fndapp.users` belongs to the User service; `apalis`
belongs to upstream task infrastructure. The explicit migration runner (`foundation-db`)
uses a dedicated `foundation_migrator` connection (selecting role `foundation_owner`) and
orchestrates migrations in order:
1. Upstream Apalis SQLx migrations run first (`apalis._sqlx_migrations`).
2. Infrastructure runtime contracts and privilege hardening apply to `infra` and `apalis`.
3. Domain SeaORM migrators (`users::pg::migrations::Migrator::up`) install domain schemas,
   tables, check constraints, grants, and readiness functions, tracking history under
   domain-qualified tables (e.g. `fndapp.seaql_migrations`).

Never edit an applied migration. Add a new one and update the matching readiness
contract. Keep DDL, functions and grants versioned within migration definitions rather than
reconciling them from Rust on each service invocation. Provisioning owns roles and initial
default privileges.

`foundation_migrator` may select `foundation_owner` for DDL. `foundation_users`
and `foundation_tasks` receive deliberately limited runtime privileges. They
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
workspace. A running Docker daemon is the only infrastructure prerequisite.
`cargo test --locked --workspace --all-features` runs every integration suite.

Each suite launcher owns one Testcontainers PostgreSQL instance. It launches its
ignored SQLx cases with a child-only `DATABASE_URL`; `sqlx::test` creates a fresh
database for every case. This avoids process-global environment mutation and
parallel-test interference. Provisioning and migrations match the runtime role
boundaries used by the application. Use synthetic data and temporary storage.

Add tests for observable behavior and meaningful failure cases. Avoid tests that
merely restate constructors or private implementation details.
