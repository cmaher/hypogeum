# Hypogeum

Read [the coding standards](docs/coding_standards.md) before changing code.

## Template naming

This is a runnable template. Every project-specific name must use the literal
forms below so `scripts/rename-project.py` can replace the complete identity.
When a project-specific prefix or namespace is needed, use these forms verbatim;
do not introduce alternate project names, abbreviations, placeholder prefixes,
or casing variants.

| Context | Required form | Example |
| --- | --- | --- |
| Lowercase identifiers and namespaces | `hypogeum` | `hypogeum.users.v1`, schema/database `hypogeum`, role `hypogeum_migrator` |
| Environment variables and uppercase identifiers | `HYPOGEUM` | `HYPOGEUM_ENV_FILE`, `HYPOGEUM_USERS_DATABASE_URL` |
| Display names and PascalCase identifiers | `Hypogeum` | Documentation title `Hypogeum` |
| Project-prefixed Cargo packages / binaries | `hypogeum-` | `hypogeum-db` |
| Project-prefixed Rust library identifiers | `hypogeum_` | `hypogeum_db` |

Keep these spellings explicit in tracked text. Do not construct the project name
from fragments, encode it, or embed it in fixed-length byte literals. Keep file
and directory names generic: the rename script replaces file contents only.
Update matching configuration, SQL grants, protobuf bindings, tests and docs when
adding a naming surface, and extend the rename tests if it needs new coverage.

Generic reusable crate names and infrastructure/upstream namespaces such as
`infra`, `apalis`, and `public` retain their names. Synthetic `EXAMPLE_` settings
may test arbitrary caller-supplied prefixes; actual project configuration must
use `HYPOGEUM_`. Customization is for fresh projects before database provisioning;
follow the README's project customization workflow.

## Development conventions

- Use plain data structs, constructor injection, and trait-based repository,
  service and client interfaces. Keep transport adapters thin.
- Repositories own application SQL. Build typed entity queries with SeaORM,
  integrating with SQLx pool ownership via `SqlxPostgresConnector`. Keep tables schema-qualified.
- Define shared errors in `crates/errors`; use `anyhow` at binary boundaries.
  Database and internal details must not enter gRPC responses.
- Binaries load environment once via `env-config` (`EnvSource::load`), deserialize
  per-binary `Config` with `envy`, use `Secret` for credentials, and never mutate
  process env. Libraries receive typed values, pools and dependencies explicitly.
- Run migrations explicitly through `hypogeum-db`; service startup checks
  readiness using restricted runtime credentials.
- Domain crates own their schema DDL, grants and readiness functions in collocated
  SeaORM migrations under `src/pg/migrations/`. `hypogeum-db` orchestrates upstream
  Apalis SQLx migrations, infrastructure contracts, and domain SeaORM migrators.
- Keep this workspace generic. Add product behavior in the consuming project.
- Keep reusable infrastructure in `crates/` and domain crates in `crates/domain/`.
  The generic User example lives in `crates/domain/users/`.
- Use Cargo Make for local Docker/Compose operations. Database integration tests
  own their containers through Testcontainers and isolate databases with SQLx.
- Run `cargo make ci` before committing. Docker must be running. No pre-test
  database scripts or ambient test database URLs are required.

`crates/test-support` is a development dependency only. Its suite launcher runs
ignored SQLx cases in a child process, ensuring the ordinary `cargo test` command
executes them and propagates failures. Do not run those cases directly against a
shared development database.
