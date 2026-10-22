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
├── tests/
│   └── browser/          # Playwright smoke checks for HTML views
├── cmd/
│   ├── app/              # HTTP server executable (app / exemplum-app-cli)
│   └── db/               # Database migration CLI (exemplum-db / exemplum-db-cli)
└── crates/
    ├── app/              # Composed HTTP router (exemplum-app)
    └── domain/
        ├── auth/         # Credential verification, sessions, and token exchange
        ├── hello-world/  # Greeting service, JSON API and HTML view (exemplum-dmn-hello-world)
        └── users/        # User entity, SeaORM migrations, gRPC (exemplum-dmn-users)
```

## Environment Configuration

Exemplum configuration uses the `EXEMPLUM_` prefix. Configuration precedence is:
**Process Environment > Dotenv file > Defaults**.

| Variable | Default | Purpose |
| --- | --- | --- |
| `EXEMPLUM_APP_BIND` | `127.0.0.1:8080` | Public HTTP server listen address |
| `EXEMPLUM_APP_DATABASE_URL` | required | Runtime connection using the `exemplum_users` role |
| `EXEMPLUM_APP_JWT_SECRET` | required | Private signing secret containing at least 32 bytes |
| `EXEMPLUM_APP_MAX_PASSWORD_VERIFICATIONS` | `4` | Maximum concurrent password checks; must be positive and fit the semaphore capacity |
| `EXEMPLUM_APP_SESSION_CLEANUP_INTERVAL_SECONDS` | `3600` | Positive interval between expired-session cleanup runs |
| `EXEMPLUM_APP_CEDAR_SCHEMA` | `config/schema.cedarschema` | Cedar schema loaded at startup |
| `EXEMPLUM_APP_CEDAR_POLICIES` | `config/policies.cedar` | Cedar policies loaded at startup |
| `EXEMPLUM_LOG` | `info` | Log level filter |
| `EXEMPLUM_LOG_FORMAT` | `text` | Logging format (`text` or `json`) |
| `EXEMPLUM_POSTGRES_PORT` | `5432` | Local PostgreSQL host port in Docker Compose |
| `EXEMPLUM_MIGRATION_DATABASE_URL` | - | Migration connection string (migrator credentials) |
| `EXEMPLUM_USERS_DATABASE_URL` | - | Runtime database connection string for Users domain |
| `EXEMPLUM_ENV_FILE` | unset | Dotenv file path selector (`""` to disable file loading) |

## Development Commands

All tasks can be executed using `cargo make` from within the `example/` directory, or from the root via `cargo make -e CARGO_MAKE_WORKING_DIRECTORY=example <task>`.

### Database Lifecycle

Database operations are executed by `exemplum-db`, which constructs an application-specific `DatabaseConfig` (defining roles `exemplum_migrator`, `exemplum_owner`, `exemplum_tasks`, schema `exemplum`, registered domain migrators, and service contracts) and drives the decoupled framework migration engine `hypgmx-rdbms`.

```sh
cargo make db-up        # Start PostgreSQL container in background
cargo make db-status    # Inspect container status
cargo make db-provision # Execute provision.sql to create roles and database
cargo make db-migrate   # Apply database migrations via exemplum-db
cargo make db-verify    # Verify database schema contracts
cargo make db-down      # Stop PostgreSQL container
```

`exemplum-db inspect --output json` reports each schema's contracts as an array
under `data.contracts.<schema>`, preserving all tables registered in that schema.

### Running the Application

```sh
# From example/, configure local database credentials:
cp env/local.env.example env/local.env
# Set EXEMPLUM_APP_JWT_SECRET in env/local.env to a private value:
openssl rand -hex 32
cargo make db-up
cargo make db-provision
cargo make db-migrate
cargo make app
```

The binary requires explicit database credentials and a signing secret. Before
binding its listener, it checks the restricted runtime role, user schema,
credential repository, and PostgreSQL session store. It never runs migrations.
Sessions survive application restarts when using the same database. Policy files
are loaded at startup; relative paths resolve from the process working directory.
Default component factories are for tests and local composition and generate
independent, ephemeral signing keys.

Authentication routes are `POST /api/auth/login`, `POST /api/auth/logout`, and
`POST /api/auth/token`. `GET /api/hello/me` accepts a session cookie or signed
Bearer token and resolves the stored user display name. Password verification
runs on blocking workers with a configurable concurrency limit (four by default)
shared by real and dummy checks. Unknown usernames still perform password verification.
Expired sessions are deleted at startup and periodically; cleanup stops when the
server finishes shutting down.
The token endpoint accepts form or JSON credentials and HTTP Basic credentials;
responses use OAuth error fields and prevent caching with `no-store`/`no-cache`.
Other endpoints retain the shared JSON error envelope.

Verify the running application:
```sh
curl http://127.0.0.1:8080/healthz
curl -X POST http://127.0.0.1:8080/api/hello -H "Content-Type: application/json" -d '{"name":"Ada"}'
curl -X POST http://127.0.0.1:8080/hello -d 'name=Ada'
```

Open `http://127.0.0.1:8080/` for the server-rendered greeting page. The form
works without JavaScript and is enhanced with the vendored HTMX served from
`/assets`. Every path under `/api` keeps the JSON error envelope; page routes
return safe HTML errors. See [HTML Views and HTMX](../docs/app.md#7-html-views-and-htmx).

### Browser Smoke Checks

`tests/browser` runs Playwright with JavaScript enabled and disabled against a
database-free server (`crates/app/examples/view_smoke_server.rs`):

```sh
cd tests/browser
npm install
npx playwright install chromium
npm test
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
