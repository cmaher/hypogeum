# Hypogeum & Exemplum

Read [the coding standards](docs/coding_standards.md) before changing code.

## Repository Architecture

This repository is organized into a two-tier architecture:

1. **Hypogeum Framework Foundation** (root workspace):
   - Foundation crates in `crates/core/` (`hypgm-*`): `envconfig`, `errors`, `http`, `logging`, `rpc`, `test-support`.
   - External driver and stateful adapters in `crates/ext/` (`hypgm-ext-*`): `file-storage`, `rdbms`, `tasks`.
   - Standalone framework operations UI: `cmd/apalis-dashboard`.
2. **Exemplum Reference Implementation** (`example/` workspace):
   - Standalone reference project demonstrating consumption of Hypogeum framework crates.
   - Independent Cargo workspace (`example/Cargo.toml`) referencing framework crates via relative path dependencies.
   - App router in `example/crates/app` (`exemplum-app`).
   - Domain crates in `example/crates/domain/`: `hello-world` (`exemplum-dmn-hello-world`), `users` (`exemplum-dmn-users`).
   - Binaries in `example/cmd/`: `app` (`exemplum-app-cli`), `db` (`exemplum-db-cli`).
   - Local database stack in `example/docker-compose.yml` and `example/infra/postgres/`.
   - Dedicated task automation in `example/Makefile.toml`.

## Template Naming & Scoped Customization

The project template rename utility (`scripts/rename-project.py`) is scoped exclusively to `example/`. It customizes the Exemplum reference application into a project-specific service without modifying the underlying Hypogeum framework crates.

| Context | Exemplum Template Form | Target Form (e.g. `acme_tools`) |
| --- | --- | --- |
| Lowercase identifiers and namespaces | `exemplum` | `acme_tools`, database/schema `acme_tools`, role `acme_tools_migrator` |
| Environment variables | `EXEMPLUM` | `ACME_TOOLS_APP_PORT`, `ACME_TOOLS_MIGRATION_DATABASE_URL` |
| Display names and PascalCase identifiers | `Exemplum` | Documentation title `AcmeTools` |
| Short library / package identifiers | `exmpl` | `acme_tools` |

Root framework crates and infrastructure namespaces (`hypgm-*`, `hypgm-ext-*`, `infra`, `apalis`, `public`) are part of the reusable framework and remain untouched by renaming.

## Development Conventions

- Use plain data structs, constructor injection, and trait-based repository, service, and client interfaces. Keep transport adapters thin.
- Repositories own application SQL. Build typed entity queries with SeaORM, integrating with SQLx pool ownership via `SqlxPostgresConnector`. Keep tables schema-qualified.
- Define shared errors and the `PublicError` / `PublicErrorInfo` contract in `crates/core/errors` (`hypgm-errors`). Domain crates define domain-specific errors implementing `PublicError`. Use `anyhow` at binary boundaries. Database and internal details must not enter HTTP or gRPC responses.
- Binaries load environment once via `hypgm-envconfig` (`crates/core/envconfig`, `EnvSource::load_for` with application prefix, or `EnvSource::load` for framework binaries), deserialize per-binary `Config` with `envy`, use `Secret` for credentials, and never mutate process env. Libraries receive typed values, pools and dependencies explicitly.
- Run migrations explicitly through `exemplum-db` (driving framework `hypgm-ext-rdbms` via `DatabaseConfig`); runtime processes never migrate on startup and check readiness using restricted credentials.
- `crates/core/test-support` is a development dependency only. Its suite launcher runs ignored SQLx cases in child processes against Testcontainers, ensuring ordinary `cargo test` commands execute them and propagate failures.

## Development Workflows & Commands

### Root Workspace Commands

From repository root:

```sh
cargo make ci          # Full CI: fmt, clippy, build, test, test-rename, and example CI
cargo make fmt         # Check root workspace formatting
cargo make fmt-fix     # Format root workspace
cargo make clippy      # Lint root workspace
cargo make test        # Run root unit and integration tests (Docker must be running)
cargo make test-rename # Run isolated Python rename test suite
cargo make apalis-dashboard # Run standalone task dashboard
```

### Exemplum Reference Project Commands

From `example/` (or via `cargo make --cwd example <task>`):

```sh
cargo make db-up        # Start local PostgreSQL container in background
cargo make db-status    # Check PostgreSQL container status
cargo make db-provision # Provision database, roles, and schema contracts
cargo make db-migrate   # Apply database migrations via exemplum-db
cargo make db-verify    # Verify database schema contracts
cargo make db-down      # Stop local PostgreSQL container
cargo make app          # Run public HTTP application (127.0.0.1:8080)
cargo make test         # Run example unit and integration tests
cargo make ci           # Run full Exemplum CI (fmt, clippy, build, test)
```
