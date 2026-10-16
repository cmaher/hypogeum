# Rust Foundation

A Rust workspace for starting services with PostgreSQL, gRPC, typed background
tasks, and streamed byte storage. A small User service demonstrates the layers.

Reusable infrastructure lives in `crates/`. Domain crates live in `crates/domain/`,
starting with the generic User example in `crates/domain/users/`.

| Crate | Responsibility |
| --- | --- |
| `users` | Plain User objects, repository, service, gRPC server and client |
| `rpc` | Generated protobuf client/server bindings |
| `foundation-db` | Explicit migrations, verification and database inspection |
| `postgres` | Constructor-injected pool configuration |
| `errors` | Shared typed errors and transport mappings |
| `tasks` | Apalis queues, workers, retry policy and Board |
| `file-storage` | Streamed immutable byte storage with integrity checks |
| `env-config` | Binary-only env snapshot and typed deserialization |
| `test-support` | Testcontainers suite bootstrap and SQLx test configuration |

## Development

Install Rust 1.94 and Docker. Optionally use [mise](https://mise.jdx.dev/) to
install the tools in `mise.toml`:

```sh
mise trust
mise install
```

With Docker running, tests need no database setup or configuration file:

```sh
cargo test --locked --workspace --all-features
```

Database suites start one PostgreSQL container each through Testcontainers.
`sqlx::test` gives each case an isolated database. An ordinary suite test launches
its SQLx cases in a child harness with `DATABASE_URL` supplied before threads
start. Cases marked ignored are executed by that launcher as part of the normal
test command; child failures fail the suite. Containers are removed afterward.

Use Cargo Make for repository commands:

```sh
cargo make ci          # format check, clippy, build, all tests
cargo make fmt-fix
cargo make test-db     # database, User transport, and task integration suites
```

GitHub Actions runs the same format, lint, build and test checks on Ubuntu.
Protobuf compilation uses `protox`; a system `protoc` is unnecessary.

## Run locally

Compose provides a local PostgreSQL instance at `127.0.0.1:5432` with administrator
login `admin` / `password`. Service roles also default to `password` locally.

```sh
cp env/local.env.example env/local.env
cargo make db-up
cargo make db-provision
cargo make db-migrate
cargo make db-verify
cargo make users
```

The User gRPC endpoint defaults to `127.0.0.1:50051`. In another terminal,
`cargo make tasks-board` serves the empty Apalis Board at `127.0.0.1:8082`.
`cargo make db-down` stops Compose while retaining its data volume.

Configuration precedence is: process env > dotenv file > defaults. The `APP_ENV_FILE`
variable selects the dotenv file to load:

| `APP_ENV_FILE` | Behavior |
| --- | --- |
| unset | Load `env/local.env` if present; silently skip if absent |
| `<path>` | File must exist, else exit with error |
| `""` (empty) | Load no file (deployment opt-out) |

Remote PostgreSQL: set `APP_MIGRATION_DATABASE_URL`, `APP_USERS_DATABASE_URL`, and
`APP_TASKS_DATABASE_URL` to connections for their respective roles and skip Compose.
Deployment: set real environment variables plus `APP_ENV_FILE=`. Set `APP_LOG`
(default `info`) for log filtering. Provision roles and database grants using the
SQL in `infra/postgres/` with administrator access, then run the explicit migration
command. Server processes never migrate.

If changing local passwords or ports, update the matching URLs as well.
Production credentials and binding policies belong in deployment configuration.

```sh
cargo run --locked -p foundation-db -- --describe
cargo run --locked -p foundation-db -- inspect --output json
```

The administration CLI returns JSON and exits nonzero on failure. Errors redact
connection details. Migration and runtime accounts are separate; runtime roles
cannot perform DDL, inspect migration ledgers, or access each other's data.

## Schema names and customization

| Schema | Purpose | Can a new project rename it? |
| --- | --- | --- |
| `fndapp` | Application tables (`fndapp.users`), readiness function, and domain migration history | Yes. This is the application-owned example schema; `fndapp` is a search-and-replace template name designed for easy customization. |
| `infra` | Infrastructure contracts, platform privilege hardening, and extensions | Yes, but keeping it is recommended. |
| `apalis` | Upstream task tables, functions, extensions and a separate migration ledger | Keep this name. The pinned Apalis implementation and migrations use schema-qualified `apalis` references. Changing our settings alone cannot rename it. |
| `public` | PostgreSQL's default schema; unused for application data | Leave it in place. Application tables belong in explicitly owned schemas. |
| `pg_catalog`, `pg_temp` | PostgreSQL system schemas used in controlled search paths | Leave these names unchanged. |

Schema names are currently fixed in code and SQL; there is no schema environment
variable. Changing a connection URL or `search_path` does not rename them:
SeaQuery repository statements and SeaORM migrations use explicit schema names.

When customizing a **fresh project before applying any migrations**:

- To rename `fndapp`, update the domain migrations in
  [`crates/domain/users/src/pg/migrations/`](crates/domain/users/src/pg/migrations/), the
  SeaORM entity in [`crates/domain/users/src/repo/entity.rs`](crates/domain/users/src/repo/entity.rs), the
  SeaQuery table references and readiness check in
  [`crates/domain/users/src/repo/postgres.rs`](crates/domain/users/src/repo/postgres.rs), and
  database verification/inspection in [`crates/db/src/`](crates/db/src/).
  Update the integration tests and documentation to match.
- To rename `infra`, update references in
  [`crates/db/src/infra.rs`](crates/db/src/infra.rs), plus qualified references in
  verification, inspection and integration tests. Retain a separate
  application domain ledger and upstream Apalis ledger.
- Keep provisioning grants and role search paths in
  [`infra/postgres/`](infra/postgres/) consistent with any schema changes. Run
  `cargo make ci` to check migrations, User CRUD and runtime privilege boundaries.

For a database that **already has migrations applied**, preserve existing
migration files and checksums. Rename application schemas through a new forward
migration and coordinate the code change. Renaming the ledger schema also needs
a coordinated move of the existing ledger before switching the migrator's
configured location. Pointing SQLx or SeaORM at a new, empty ledger would make existing
migrations appear unapplied.

The database name (`foundation` in local Compose) is separate from these schemas;
the path in a PostgreSQL URL selects the database. Names such as
`foundation_owner`, `foundation_migrator`, `foundation_users` and `foundation_tasks`
are roles, also separate from schema names. Renaming roles requires matching
changes to provisioning, grants, credential checks, connection URLs and tests.

The `APP_` environment prefix and `fndapp` schema name are template customization
points. Variable names and schema identifiers are written out literally, so a consuming
project can customize them with a workspace-wide search and replace of `APP_` and `fndapp`
across Rust sources, `env/local.env.example`, `docker-compose.yml`, `Makefile.toml`,
and documentation.

## User example

`User` contains a UUID, display name, and creation/update timestamps. The API
supports create, get, list, rename and delete. Names are trimmed and accept
1–200 Unicode characters. Lists use UUID ordering, an exclusive cursor, a
default page size of 50, and a maximum of 100. The server generates IDs and
PostgreSQL supplies timestamps. See `proto/users.proto` and the integration tests
in `crates/domain/users/tests/cases/` for the complete contract.

The example leaves authentication and application-specific permissions to the
project built from it. Keep development endpoints bound to loopback until a
deployment supplies its access controls.

See [architecture and coding standards](docs/coding_standards.md),
[tasks](docs/tasks.md), and [file storage](docs/file-storage.md).
