# Hypogeum

A modular Rust framework and reference architecture for building robust backend services with PostgreSQL, internal gRPC, composed Axum JSON APIs, typed background tasks, and streamed byte storage.

## Architecture

The repository uses a two-tier architecture cleanly separating framework foundations from application domain code:

```text
/workspace/
├── Cargo.toml                    # Root workspace: framework crates + operations tools
├── Cargo.lock
├── Makefile.toml                 # Root CI orchestrator: runs root CI + example CI
├── CLAUDE.md                     # Development conventions & architecture guide
├── cmd/
│   └── apalis-dashboard/         # Standalone framework background tasks dashboard
├── crates/
│   ├── core/                     # Core runtime & cross-cutting foundations (hypgm-*)
│   │   ├── envconfig/            # Type-safe environment loading and secrets
│   │   ├── errors/               # Shared typed error types and PublicError trait
│   │   ├── http/                 # Axum middleware, error mapping, and body limits
│   │   ├── logging/              # Structured tracing subscriber configuration
│   │   ├── rpc/                  # Generated gRPC protobuf client and server definitions
│   │   ├── test-support/         # Testcontainers bootstrap for isolated database tests
│   │   └── view/                 # Opt-in HTML views, HTMX support, and vendored assets
│   └── ext/                      # External driver & stateful adapters (hypgmx-*)
│       ├── file-storage/         # Streamed byte storage at path-shaped keys
│       ├── rdbms/
│       │   ├── db/               # Migration runner, schema verification, inspection
│       │   └── postgres/         # Database pool management
│       └── tasks/                # Apalis PostgreSQL queues, workers, and boards
├── scripts/
│   ├── rename-project.py         # Template customizer scoped exclusively to example/
│   └── tests/
│       └── test_rename_project.py
└── example/                      # Exemplum reference project (independent workspace)
    ├── Cargo.toml                # Example workspace manifest
    ├── Makefile.toml             # Example tasks (db-up, db-migrate, app, test, ci)
    ├── README.md                 # Exemplum documentation & onboarding guide
    ├── docker-compose.yml        # Local PostgreSQL container for Exemplum
    ├── infra/                    # PostgreSQL init & schema provisioning
    ├── cmd/                      # Exemplum binaries (app, exemplum-db)
    └── crates/                   # Exemplum libraries (exemplum-app, exemplum-dmn-*)
```

## Exemplum Reference Application

The `example/` directory houses **Exemplum**, an independent, self-contained workspace demonstrating consumption of the Hypogeum framework crates. Exemplum includes:
- User CRUD service over SeaORM with gRPC interfaces (`exemplum-dmn-users`)
- Public greeting API and composed Axum router (`exemplum-app`, `exemplum-dmn-hello-world`)
- Executable entry points for the HTTP server (`app`) and database migration tool (`exemplum-db`)
- Local PostgreSQL Docker Compose stack and dedicated `Makefile.toml`

For detailed setup, environment variables, and local execution instructions, see [example/README.md](example/README.md).

## Project Customization

The script `scripts/rename-project.py` customizes the `example/` reference application into a project-specific service or template repository, without modifying the framework foundation crates:

1. Preview affected files and replacements:
   ```sh
   python3 scripts/rename-project.py acme_tools --dry-run
   ```

2. Apply rename to `example/`:
   ```sh
   python3 scripts/rename-project.py acme_tools
   cargo make --cwd example fmt-fix
   ```

3. Verify customized project:
   ```sh
   cargo make --cwd example ci
   ```

## Development & Verification

Install Rust 1.94, Docker, Git, and Python 3.9+. With Docker running, all unit and integration tests run self-contained without pre-provisioning databases:

```sh
# Root framework verification (runs fmt, clippy, test, test-rename, example CI, and sweep):
cargo make ci

# Run only root workspace unit and integration tests:
cargo make test

# Run python rename tests:
cargo make test-rename
```

Database integration tests automatically provision isolated PostgreSQL containers via Testcontainers and SQLx.

### Build performance and disk usage

The workspace ships defaults that keep builds fast and `target/` bounded; the
Exemplum template carries the same settings so new projects start with them.

- **One compile per mode.** `cargo make ci` runs clippy (check mode) and then
  `cargo test`, which builds every library, binary, and test target. There is
  no separate `build` step (`cargo make build` remains for manual use). Every
  pipeline invocation uses `--locked --workspace --all-features`, because a
  different feature set makes cargo rebuild dependents instead of reusing them.
- **Dev profile.** `[profile.dev]` keeps line tables in workspace crates for
  backtraces and drops debug info from dependencies. `strip = "none"` is set
  alongside it because macOS refuses to load stripped proc-macro dylibs
  ("mis-aligned LINKEDIT string pool").
- **No incremental compilation.** `.cargo/config.toml` sets
  `incremental = false`. Incremental data under `target/debug/incremental` is
  never pruned and every flag or feature change leaves another copy; it also
  stops sccache from caching workspace crates. The tradeoff is a slower
  edit-rebuild of a single large crate in exchange for bounded disk use and
  cache hits.
- **CI.** When cargo-make detects CI, it also sets `CARGO_INCREMENTAL=0`,
  `CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_DEV_STRIP=none`.
- **Shared target directory.** The root `.cargo/config.toml` sets
  `target-dir = "target"`; cargo also reads it inside `example/`, so the
  framework and Exemplum workspaces share `target/` and reuse dependencies.
- **Pruning.** Cargo never prunes `target/`. `cargo make sweep` removes
  artifacts unused for 7 days with [cargo-sweep](https://github.com/holmgr/cargo-sweep)
  (installed by `mise.toml`, or `cargo install cargo-sweep`) and runs at the end
  of `cargo make ci`; it is skipped when cargo-sweep is not installed. Run `cargo clean` occasionally to
  drop stale copies of large dependency rlibs in `target/debug/deps`.

[sccache](https://github.com/mozilla/sccache) is the recommended compiler
cache (`RUSTC_WRAPPER=sccache`). Size and key it deliberately:

- The default cache cap is 10 GiB, and a full cache evicts constantly. Raise it
  with `SCCACHE_CACHE_SIZE` (for example `SCCACHE_CACHE_SIZE=50G`).
- Entries are keyed on absolute paths, so separate checkouts or worktrees miss
  each other's entries unless `SCCACHE_BASEDIRS` lists their parent directories.
- `SCCACHE_IDLE_TIMEOUT=0` keeps the server alive so `sccache --show-stats`
  reports totals across builds.
