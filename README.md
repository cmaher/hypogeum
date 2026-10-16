# Hypogeum

A Rust workspace for starting services with PostgreSQL, gRPC, typed background
tasks, and streamed byte storage. User CRUD and a public HelloWorld service
demonstrate the layers.

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
| `hello-world` | Database-independent greeting service and public Tonic adapter |
| `web-server` | One listener for HTTP health, native gRPC and gRPC-Web |
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

## Public HelloWorld example

```sh
cargo make web-server
```

This server needs no database, migrations, Docker, or database configuration.
`HYPOGEUM_WEB_BIND` defaults to `127.0.0.1:8080`. It uses the same environment-file
precedence and `HYPOGEUM_ENV_FILE` behavior described above; `HYPOGEUM_ENV_FILE=`
disables file loading. Ctrl-C and SIGTERM drain active requests before exit.

One socket serves all of these:

| Route / protocol | Client behavior |
| --- | --- |
| `GET /healthz`, ordinary HTTP | HTTP 200 with text `ok` |
| `/hypogeum.public.v1.PublicHelloWorld/SayHello`, native gRPC | Generated Rust Tonic client using HTTP/2 |
| The same RPC path, gRPC-Web | Browser protobuf client using binary or base64 text framing over HTTP/1.1 or HTTP/2 |

`proto/public_hello_world.proto` defines the unary request and reply. Names are
trimmed; blank greets `world`. Names may contain up to 200 Unicode scalar values
and must not contain NUL. The response is `Hello, {name}!`. Validation reports
`InvalidArgument`. Clients must check RPC statuses, including gRPC-Web trailer
frames or trailers-only status headers; HTTP 200 alone does not mean success.
There is no JSON transcoding or frontend build step.

Only PublicHelloWorld is registered on this public listener. User CRUD remains
on `127.0.0.1:50051`; the task Board remains on `127.0.0.1:8082`. CORS applies only
to registered public RPC routes: any origin, no credentials, POST/OPTIONS, and
gRPC-Web request/status headers. Health and unregistered services receive no CORS
permission. Keep the default loopback bind for local development.

Axum enables HTTP/2 explicitly and merges ordinary routes with Tonic
[`Routes::into_axum_router`](https://docs.rs/tonic/0.14.6/tonic/service/struct.Routes.html).
[`tonic-web`](https://docs.rs/tonic-web/0.14.6/tonic_web/) wraps the public service
for browser framing and passes native HTTP/2 gRPC through. A deployment must
provide TLS/ALPN at its ingress and preserve HTTP/2 forwarding for native gRPC,
along with gRPC-Web content types, bodies and status headers for browser clients.
TLS and ingress configuration are external to this example.

To add another public service, define its protobuf API and injected plain service,
then explicitly register its Tonic adapter with a gRPC-Web layer in
`crates/web-server/src/lib.rs` before the public CORS layer. Add loopback tests for
both native and browser calls. Registration in another server does not make a
service public. Workspace CI automatically runs the new crates and their tests;
HelloWorld tests themselves need no database, while full CI requires Docker.

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
