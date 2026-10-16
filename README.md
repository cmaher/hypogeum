# Hypogeum

A Rust workspace for starting services with PostgreSQL, gRPC, typed background
tasks, and streamed byte storage. A small User service demonstrates the layers.

Reusable infrastructure lives in `crates/`. Domain crates live in `crates/domain/`,
starting with the generic User example in `crates/domain/users/`.

| Crate | Responsibility |
| --- | --- |
| `users` | Plain User objects, repository, service, gRPC server and client |
| `rpc` | Generated protobuf client/server bindings |
| `hypogeum-db` | Explicit migrations, verification and database inspection |
| `postgres` | Constructor-injected pool configuration |
| `errors` | Shared typed errors and transport mappings |
| `tasks` | Apalis queues, workers, retry policy and Board |
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
`cargo make tasks-board` serves the empty Apalis Board at `127.0.0.1:8082`.
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
cargo run --locked -p hypogeum-db -- --describe
cargo run --locked -p hypogeum-db -- inspect --output json
```

The administration CLI returns JSON and exits nonzero on failure. Errors redact
connection details. Migration and runtime accounts are separate; runtime roles
cannot perform DDL, inspect migration ledgers, or access each other's data.

## Project customization

Hypogeum is a runnable template. Its project-specific names use three literal,
searchable forms: `hypogeum`, `HYPOGEUM`, and `Hypogeum`. Customize a fresh checkout
**before provisioning a database or applying migrations**:

```sh
python3 scripts/rename-project.py acme_tools --dry-run
python3 scripts/rename-project.py acme_tools
cargo make fmt-fix
cargo make ci
```

The script requires Git and Python 3.9 or newer, with no extra Python packages.
It works from any current directory when invoked by its path. Use `--output json`
for structured results or errors, and `--describe` for its command schema.
`cargo make test-rename` runs its isolated behavioral tests.

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
resulting diff. Repository hosting and tracker project renames are separate steps.

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
