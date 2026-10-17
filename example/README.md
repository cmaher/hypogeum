# Exemplum Reference Application

Exemplum is the reference project demonstrating and validating consumption of the Hypogeum framework crates (`hypgm-*`). It provides a complete, runnable service stack including domain business logic, HTTP API routing, database migrations, and local Docker infrastructure.

## Project Structure

```text
example/
├── Cargo.toml            # Independent Cargo workspace manifest
├── Makefile.toml         # Development and CI task automation
├── README.md             # This documentation
├── docker-compose.yml    # Local PostgreSQL service container
├── infra/
│   └── postgres/
│       ├── provision.sql # Database, role, and permission provisioning
│       └── database.sql  # Database-level grants and defaults
├── cmd/
│   ├── app/              # HTTP server executable (app / exemplum-app-cli)
│   └── db/               # Database migration CLI (exemplum-db / exemplum-db-cli)
└── crates/
    ├── app/              # Composed HTTP router (exemplum-app)
    └── domain/
        ├── hello-world/  # In-memory greeting service (exemplum-dmn-hello-world)
        └── users/        # User entity, SeaORM migrations, gRPC (exemplum-dmn-users)
```

## Environment Configuration

Exemplum configuration uses the `EXEMPLUM_` prefix. Configuration precedence is:
**Process Environment > Dotenv file > Defaults**.

| Variable | Default | Purpose |
| --- | --- | --- |
| `EXEMPLUM_APP_BIND` | `127.0.0.1:8080` | Public HTTP server listen address |
| `EXEMPLUM_LOG` | `info` | Log level filter |
| `EXEMPLUM_LOG_FORMAT` | `text` | Logging format (`text` or `json`) |
| `EXEMPLUM_POSTGRES_PORT` | `5432` | Local PostgreSQL host port in Docker Compose |
| `EXEMPLUM_MIGRATION_DATABASE_URL` | - | Migration connection string (migrator credentials) |
| `EXEMPLUM_USERS_DATABASE_URL` | - | Runtime database connection string for Users domain |
| `EXEMPLUM_ENV_FILE` | unset | Dotenv file path selector (`""` to disable file loading) |

## Development Commands

All tasks can be executed using `cargo make` from within the `example/` directory, or from the root via `cargo make -e CARGO_MAKE_WORKING_DIRECTORY=example <task>`.

### Database Lifecycle

Database operations are executed by `exemplum-db`, which constructs an application-specific `DatabaseConfig` (defining roles `exemplum_migrator`, `exemplum_owner`, `exemplum_tasks`, schema `exemplum`, registered domain migrators, and service contracts) and drives the decoupled framework migration engine `hypgm-ext-rdbms`.

```sh
cargo make db-up        # Start PostgreSQL container in background
cargo make db-status    # Inspect container status
cargo make db-provision # Execute provision.sql to create roles and database
cargo make db-migrate   # Apply database migrations via exemplum-db
cargo make db-verify    # Verify database schema contracts
cargo make db-down      # Stop PostgreSQL container
```

### Running the Application

```sh
# Run the public HTTP application (binds to 127.0.0.1:8080 by default):
cargo make app
```

Verify the running application:
```sh
curl http://127.0.0.1:8080/healthz
curl -X POST http://127.0.0.1:8080/api/hello -H "Content-Type: application/json" -d '{"name":"Ada"}'
```

### Verification & CI

```sh
cargo make fmt          # Verify source code formatting
cargo make fmt-fix      # Automatically format source code
cargo make clippy       # Run clippy with -D warnings
cargo make build        # Build all workspace packages
cargo make test         # Run unit and integration tests (Docker must be running)
cargo make ci           # Run full CI suite (fmt, clippy, build, test)
```

## Template Customization

Exemplum serves as the target template for new project generation. The repository-level script `scripts/rename-project.py` is scoped to inspect and customize files within `example/`, replacing `exemplum` / `Exemplum` identities with your project's chosen identity.
