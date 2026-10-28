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
# Root framework verification (runs fmt, clippy, build, test, test-rename, and example CI):
cargo make ci

# Run only root workspace unit and integration tests:
cargo make test

# Run python rename tests:
cargo make test-rename
```

Database integration tests automatically provision isolated PostgreSQL containers via Testcontainers and SQLx.
