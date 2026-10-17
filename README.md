# Hypogeum

A Rust workspace for starting services with PostgreSQL, internal gRPC,
composed Axum JSON APIs, typed background tasks, and streamed byte storage.
User CRUD and a public HelloWorld service demonstrate the layers.

## Project customization

Start with a fresh Git checkout and customize it **before provisioning a database
or applying migrations**. Run the commands below from the repository root,
replacing `acme_tools` with your project's lowercase identifier. The rename script
needs Git and Python 3.9 or newer, with no extra Python packages. Install the
[development tools](#development) and start Docker before running CI.

1. Create a branch and preview the affected files and replacement counts. Preview
   makes no changes:

   ```sh
   git switch -c customize-project
   python3 scripts/rename-project.py acme_tools --dry-run
   ```

2. Apply the rename, format Rust sources, and review the resulting diff:

   ```sh
   python3 scripts/rename-project.py acme_tools
   cargo make fmt-fix
   git diff
   ```

3. Verify the customized workspace:

   ```sh
   cargo make ci
   ```

4. Copy the renamed configuration example, then follow [Run locally](#run-locally)
   to provision a fresh database and start the services:

   ```sh
   cp env/local.env.example env/local.env
   ```

Hypogeum uses three literal, searchable forms: `hypogeum`, `HYPOGEUM`, and
`Hypogeum`. The script replaces them throughout tracked source, configuration,
tests and documentation. It also updates itself to use your new project identity.
Repository hosting and tracker project renames are separate steps.

For automation, preview with JSON output or inspect the command schema:

```sh
python3 scripts/rename-project.py acme_tools --dry-run --output json
python3 scripts/rename-project.py --describe
```

The script works from any current directory when invoked by its path.
`cargo make test-rename` runs its isolated behavioral tests.

### Naming and replacement scope

Choose lowercase ASCII words separated by single underscores. The input
`acme_tools` produces `acme_tools`, `ACME_TOOLS`, and `AcmeTools`; for example:

| Template name | Customized name |
| --- | --- |
| `hypogeum-db` Cargo package / CLI | `acme_tools-db` |
| `hypogeum_db` Rust library | `acme_tools_db` |
| `hypogeum.users.v1` protobuf namespace | `acme_tools.users.v1` |
| `hypogeum` application schema / local database | `acme_tools` |
| `hypogeum_migrator` database role | `acme_tools_migrator` |
| `HYPOGEUM_ENV_FILE` environment selector | `ACME_TOOLS_ENV_FILE` |
| `Hypogeum` display name | `AcmeTools` |

Names must produce three distinct case forms and cannot contain the current
project identity. Reserved language keywords, infrastructure schema names, and
PostgreSQL's `pg_` prefix are rejected. Project-prefixed database identifiers,
including their suffixes, must fit PostgreSQL's 63-byte identifier limit.

The script previews or updates **Git-tracked UTF-8 file contents**, including
hidden configuration, tests, `Cargo.lock`, documentation, and itself. It replaces
occurrences within longer identifiers and preserves line endings and file
permissions. The customized script uses the new identity for future renames.
File/directory names, Git history and remotes are outside its scope.

All input and file checks finish before writes begin. Unsupported casing,
matching binary content, matching symlink targets, changed file types, and
missing tracked files cause preflight failure with no writes. Symlinks are never
followed. An operating-system error during application may leave earlier files
updated; review the Git diff before retrying. Output contains paths and counts,
without printing file contents or credentials.

Ignored and untracked files are excluded. Recreate `env/local.env` from the
updated example, or update your local overrides separately. Rust formatting can
change with identifier length, so run `cargo make fmt-fix` before CI. Review the
resulting diff.

### Database schemas

| Schema | Purpose |
| --- | --- |
| `hypogeum` | Application tables (`hypogeum.users`), readiness function, and domain migration history |
| `infra` | Infrastructure contracts, platform privilege hardening, and extensions |
| `apalis` | Upstream task tables, functions, extensions and a separate migration ledger |
| `public` | PostgreSQL's default schema; unused for application data |
| `pg_catalog`, `pg_temp` | PostgreSQL system schemas used in controlled search paths |

The script renames the application schema along with database names, roles,
configuration and code. Infrastructure schemas retain their names. In particular,
the pinned Apalis implementation uses schema-qualified `apalis` references.
Schemas are explicit in code and SQL; changing a connection URL or `search_path`
does not rename them. The database name in the connection URL and PostgreSQL
roles are distinct from schema names, even when they share the project prefix.

For an existing database, preserve applied migration source and checksums. Schema,
role and migration-ledger renames need a coordinated forward migration and code
change; this script does not migrate deployed databases. Pointing a migrator at
an empty ledger would make existing migrations appear unapplied.

## Workspace overview

Reusable infrastructure lives in `crates/`. Domain crates live in `crates/domain/`,
starting with the generic User example in `crates/domain/users/`.

| Crate | Responsibility |
| --- | --- |
| `users` | Plain User objects, repository, service, gRPC server and client |
| `hello-world` | Database-independent greeting service and thin JSON controller |
| `app` | Composed public router, health endpoint, and domain mounts |
| `app-cli` (`cmd/app`) | Public app executable configuration, logging and startup |
| `http-support` | Shared Axum error mapping, 16KiB JSON extraction, and request tracing |
| `logging` | Typed subscriber builder and tracing subscriber initialization |
| `rpc` | Generated protobuf client/server bindings (internal Users RPC) |
| `hypogeum-db` | Reusable migrations, verification and database inspection |
| `hypogeum-db-cli` (`cmd/db`) | The `hypogeum-db` administration executable |
| `postgres` | Constructor-injected pool configuration |
| `errors` | Shared typed errors and transport mappings |
| `tasks` | Apalis queues, workers, retry policy and Board |
| `apalis-dashboard` (`cmd/apalis-dashboard`) | Standalone Apalis dashboard and API |
| `file-storage` | Streamed immutable byte storage with integrity checks |
| `env-config` | Binary-only env snapshot and typed deserialization |
| `test-support` | Testcontainers suite bootstrap and SQLx test configuration |

## Development

Install Rust 1.94, Docker, Git and Python 3.9 or newer. Optionally use [mise](https://mise.jdx.dev/) to
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
cargo make ci          # format check, clippy, build, Rust and Python tests
cargo make fmt-fix
cargo make test-db     # database, User transport, and task integration suites
```

GitHub Actions runs the same format, lint, build, Rust and Python checks on Ubuntu.
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
`cargo make apalis-dashboard` serves the empty Apalis Board at `127.0.0.1:8082`.
`cargo make db-down` stops Compose while retaining its data volume.

Configuration precedence is: process env > dotenv file > defaults. The `HYPOGEUM_ENV_FILE`
variable selects the dotenv file to load:

| `HYPOGEUM_ENV_FILE` | Behavior |
| --- | --- |
| unset | Load `env/local.env` if present; silently skip if absent |
| `<path>` | File must exist, else exit with error |
| `""` (empty) | Load no file (deployment opt-out) |

Remote PostgreSQL: set `HYPOGEUM_MIGRATION_DATABASE_URL`, `HYPOGEUM_USERS_DATABASE_URL`, and
`HYPOGEUM_TASKS_DATABASE_URL` to connections for their respective roles and skip Compose.
Deployment: set real environment variables plus `HYPOGEUM_ENV_FILE=`. Set `HYPOGEUM_LOG`
(default `info`) for log filtering. Provision roles and database grants using the
SQL in `infra/postgres/` with administrator access, then run the explicit migration
command. Server processes never migrate.

If changing local passwords or ports, update the matching URLs as well.
Production credentials and binding policies belong in deployment configuration.

```sh
cargo run --locked -p hypogeum-db-cli --bin hypogeum-db -- --describe
cargo run --locked -p hypogeum-db-cli --bin hypogeum-db -- inspect --output json
```

The administration CLI returns JSON and exits nonzero on failure. Errors redact
connection details. Migration and runtime accounts are separate; runtime roles
cannot perform DDL, inspect migration ledgers, or access each other's data.

## Public App and HelloWorld example

```sh
cargo make app
```

The executable entry point lives in `cmd/app/src/main.rs`; the composed router
lives in `crates/domain/app`. This server needs no database, migrations, Docker,
or database configuration.

`HYPOGEUM_APP_BIND` defaults to `127.0.0.1:8080`. Logging is configured via
`HYPOGEUM_LOG` (default `info`) and `HYPOGEUM_LOG_FORMAT` (`text` or `json`,
default `text`). It uses the same environment-file precedence and
`HYPOGEUM_ENV_FILE` behavior described above; `HYPOGEUM_ENV_FILE=` disables file
loading. Ctrl-C and SIGTERM drain active requests before exit.

Composed HTTP routes:

| Route / method | Request payload | Response behavior |
| --- | --- | --- |
| `GET /healthz` | None | HTTP 200 `text/plain; charset=utf-8` body `ok` |
| `HEAD /healthz` | None | HTTP 200 with empty body and identical headers |
| `POST /api/hello` | JSON `{"name": "Ada"}` | HTTP 200 `{"message": "Hello, Ada!"}` |

Key features:
- **16KiB Body Limit:** Raw request payloads are bounded to 16KiB; exceeding bodies receive `413 Payload Too Large`.
- **Typed Error Envelope:** All errors return uniform JSON (`{"code": "...", "message": "..."}`).
- **Request Tracing:** Generates a UUID v4 `X-Request-Id` header on every response, rejects untrusted caller IDs, and logs exactly one structured completion event with duration and status.
- **Strict Method Routing:** Mismatched methods return JSON `405 Method Not Allowed` with valid `Allow` headers.
- **Trailing Slash Discipline:** `/api/hello` is canonical; `/api/hello/` returns JSON `404 Not Found` with no redirect.

See [docs/app.md](docs/app.md) for full architectural documentation, route nesting, error mapping, and steps to add new domain endpoints.

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

See [app architecture](docs/app.md), [architecture and coding standards](docs/coding_standards.md),
[tasks](docs/tasks.md), and [file storage](docs/file-storage.md).
