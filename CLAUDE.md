# Rust Foundation

Read [the coding standards](docs/coding_standards.md) before changing code.

- Use plain data structs, constructor injection, and trait-based repository,
  service and client interfaces. Keep transport adapters thin.
- Repositories own application SQL. Build parameterized statements with SeaQuery
  and execute/map them with SQLx. Keep tables schema-qualified.
- Define shared errors in `crates/errors`; use `anyhow` at binary boundaries.
  Database and internal details must not enter gRPC responses.
- Read environment/configuration only in binary composition roots. Libraries
  receive configuration, pools and dependencies explicitly.
- Run migrations explicitly through `foundation-db`; service startup checks
  readiness using restricted runtime credentials.
- Keep this workspace generic. Add product behavior in the consuming project.
- Use Cargo Make for local Docker/Compose operations. Database integration tests
  own their containers through Testcontainers and isolate databases with SQLx.
- Run `cargo make ci` before committing. Docker must be running. No pre-test
  database scripts or ambient test database URLs are required.

`crates/test-support` is a development dependency only. Its suite launcher runs
ignored SQLx cases in a child process, ensuring the ordinary `cargo test` command
executes them and propagates failures. Do not run those cases directly against a
shared development database.
