# Rust Foundation

A Rust workspace for starting services with PostgreSQL, gRPC, typed background
tasks, and streamed byte storage. A small User service demonstrates the layers.

| Crate | Responsibility |
| --- | --- |
| `users` | Plain User objects, repository, service, gRPC server and client |
| `rpc` | Generated protobuf client/server bindings |
| `foundation-db` | Explicit migrations, verification and database inspection |
| `postgres` | Constructor-injected pool configuration |
| `errors` | Shared typed errors and transport mappings |
| `tasks` | Apalis queues, workers, retry policy and Board |
| `file-storage` | Streamed immutable byte storage with integrity checks |
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
cp env/local.secrets.example env/local.secrets
cargo make db-up
cargo make db-provision
cargo make db-migrate
cargo make db-verify
cargo make users
```

The User gRPC endpoint defaults to `127.0.0.1:50051`. In another terminal,
`cargo make tasks-board` serves the empty Apalis Board at `127.0.0.1:8082`.
`cargo make db-down` stops Compose while retaining its data volume.

Environment variables override the optional `env/local.secrets` file. Set
`FOUNDATION_LOCAL_CONFIG` to choose another file. The database can be remote:
set `MIGRATION_DATABASE_URL`, `USERS_DATABASE_URL` and `TASKS_DATABASE_URL` to
connections for their respective roles and skip Compose. Provision roles and
database grants using the SQL in `infra/postgres/` with administrator access,
then run the explicit migration command. Server processes never migrate.

If changing local passwords or ports, update the matching URLs as well.
Production credentials and binding policies belong in deployment configuration.

```sh
cargo run --locked -p foundation-db -- --describe
cargo run --locked -p foundation-db -- inspect --output json
```

The administration CLI returns JSON and exits nonzero on failure. Errors redact
connection details. Migration and runtime accounts are separate; runtime roles
cannot perform DDL, inspect migration ledgers, or access each other's data.

## User example

`User` contains a UUID, display name, and creation/update timestamps. The API
supports create, get, list, rename and delete. Names are trimmed and accept
1–200 Unicode characters. Lists use UUID ordering, an exclusive cursor, a
default page size of 50, and a maximum of 100. The server generates IDs and
PostgreSQL supplies timestamps. See `proto/users.proto` and the integration tests
in `crates/users/tests/cases/` for the complete contract.

The example leaves authentication and application-specific permissions to the
project built from it. Keep development endpoints bound to loopback until a
deployment supplies its access controls.

See [architecture and coding standards](docs/coding_standards.md),
[tasks](docs/tasks.md), and [file storage](docs/file-storage.md).
