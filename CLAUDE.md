# Rust Foundation

Read [the coding standards](docs/coding_standards.md) before changing code.

- Use plain data structs, constructor injection, and trait-based repository,
  service and client interfaces. Keep transport adapters thin.
- Repositories own application SQL. Build typed entity queries with SeaORM,
  integrating with SQLx pool ownership via `SqlxPostgresConnector`. Keep tables schema-qualified.
- Define shared errors in `crates/errors`; use `anyhow` at binary boundaries.
  Database and internal details must not enter gRPC responses.
- Binaries load environment once via `env-config` (`EnvSource::load`), deserialize
  per-binary `Config` with `envy`, use `Secret` for credentials, and never mutate
  process env. Libraries receive typed values, pools and dependencies explicitly.
- Run migrations explicitly through `foundation-db`; service startup checks
  readiness using restricted runtime credentials.
- Domain crates own their schema DDL, grants and readiness functions in collocated
  SeaORM migrations under `src/pg/migrations/`. `foundation-db` orchestrates upstream
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
