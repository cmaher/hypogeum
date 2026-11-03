# Exemplum Reference Application

Exemplum is the reference project demonstrating and validating consumption of the Hypogeum framework crates (`hypgm-*`). It provides a complete, runnable service stack including domain business logic, HTTP API routing, database migrations, and local Docker infrastructure.

## Project Structure

```text
example/
├── Cargo.toml            # Independent Cargo workspace manifest
├── Makefile              # Development and CI task automation
├── README.md             # This documentation
├── docker-compose.yml    # Local PostgreSQL, Ory Kratos and Caddy
├── env/local.env.example # Local configuration; copy to env/local.env before running
├── infra/
│   ├── caddy/Caddyfile   # Single browser origin: /.ory/kratos/* to Kratos, the rest to the app
│   ├── kratos/           # kratos.yml and identity.schema.json (username + display_name)
│   └── postgres/
│       ├── provision.sql # Roles, database, and permission provisioning
│       ├── database.sql  # Database-level grants and defaults
│       └── kratos.sql    # Kratos login role and its own database
├── tests/
│   └── browser/          # Playwright smoke checks (HTML views; Kratos auth through Caddy)
├── cmd/
│   ├── admin/            # Administrator bootstrap CLI (exemplum-admin / exemplum-admin-cli)
│   ├── app/              # HTTP server executable (app / exemplum-app-cli)
│   └── db/               # Database migration CLI (exemplum-db / exemplum-db-cli)
└── crates/
    ├── app/              # Composed HTTP router (exemplum-app)
    └── domain/
        ├── admin/        # Account approval pages and JSON API (exemplum-dmn-admin)
        ├── auth/         # Kratos session gate, flow pages, logout (exemplum-dmn-auth)
        ├── hello-world/  # Greeting service, JSON API and HTML view (exemplum-dmn-hello-world)
        └── users/        # Accounts, SeaORM migrations, gRPC (exemplum-dmn-users)
```

## Authentication architecture

[Ory Kratos](https://www.ory.sh/kratos/) (v1.3.1) owns identities, password
credentials and hashing, sessions, and the self-service login, registration and
settings flows. Caddy puts Kratos and the app on one browser origin so the Kratos
session cookie reaches both; it only routes and has no security role.

```text
browser → Caddy :8080 ─┬─ /.ory/kratos/*  → Kratos public API :4433
                       └─ everything else → app (host, 127.0.0.1:3000)
app ── server-side ──→ Kratos :4433 /sessions/whoami (forwards only ory_kratos_session)
```

- Identities sign in with **username + password**. Recovery, verification, MFA,
  social login and mail are disabled. Registration is one form (traits and
  password) and signs the user in immediately.
- The app's `session_gate` runs on every route. Without an `ory_kratos_session`
  cookie the request is anonymous. Otherwise it calls whoami; an active session
  provisions the account row on first sight (`exemplum.users`, keyed by the Kratos
  identity id, with `username` and `display_name` copied from the traits). If
  Kratos or the database is unavailable the request fails closed with a 500.
- Approval status and `is_admin` live in the app database. New accounts are
  pending: `/api` answers `401` and pages redirect to `/registration-pending`
  until an administrator approves them through `/admin/users`,
  `POST /api/admin/users/{id}/approve`, or `exemplum-admin promote`.
- Pages `/login`, `/registration` and `/settings` render the Kratos flow UI nodes
  with Askama; forms post straight to Kratos, which checks CSRF and redirects
  back. `/error?id=` shows a Kratos error message, and `POST /logout` redirects
  to the logout URL Kratos returns.
- Kratos never calls the app. The app follows or renders only URLs beneath
  `EXEMPLUM_APP_KRATOS_BROWSER_URL`.

### Known limits

- **Traits are immutable.** The Kratos `profile` settings method is off, so
  settings only change the password and the copied username and display name
  never drift.
- **Secure cookies over http.** Kratos runs without `--dev`, so its cookies are
  `Secure`. Chromium and Firefox accept them on `http://localhost`; Safari does
  not, so sign-in does not stick there. Browse `localhost`, not `127.0.0.1`.
- **Fixed origin.** `infra/kratos/kratos.yml` hardcodes `http://localhost:8080`
  (base URL, flow UI URLs, allowed return URLs). Changing `EXEMPLUM_CADDY_PORT`
  needs the same change there.
- **Linux hosts.** Caddy reaches the app through `host.docker.internal`, mapped to
  `host-gateway`. On Linux that is the Docker bridge address, which cannot reach
  an app bound to `127.0.0.1`; bind the app to the bridge address instead (for
  example `EXEMPLUM_APP_BIND=172.17.0.1:3000`). Docker Desktop and OrbStack
  forward to the host loopback, so the default works on macOS.

## Environment Configuration

Exemplum configuration uses the `EXEMPLUM_` prefix. Configuration precedence is:
**Process Environment > Dotenv file > Defaults**. `env/local.env.example` lists
the local values. Copy it to `env/local.env` before running anything locally: the
database URLs (`EXEMPLUM_MIGRATION_DATABASE_URL`, `EXEMPLUM_APP_DATABASE_URL`) have
no defaults, so `make db-migrate` and `make app` fail without them.

| Variable | Default | Purpose |
| --- | --- | --- |
| `EXEMPLUM_APP_BIND` | `127.0.0.1:3000` | App listen address (Caddy proxies to it) |
| `EXEMPLUM_APP_DATABASE_URL` | required | Runtime connection using the `exemplum_users` role |
| `EXEMPLUM_APP_KRATOS_PUBLIC_URL` | `http://127.0.0.1:4433` | Kratos public API as reached by the app (whoami, flows) |
| `EXEMPLUM_APP_KRATOS_BROWSER_URL` | `http://localhost:8080/.ory/kratos` | Kratos public API as reached by browsers through Caddy |
| `EXEMPLUM_APP_KRATOS_TIMEOUT_SECONDS` | `5` | Positive timeout for each Kratos request |
| `EXEMPLUM_ADMIN_DATABASE_URL` | required by `exemplum-admin` | Restricted `exemplum_users` connection for administrator bootstrap |
| `EXEMPLUM_MIGRATION_DATABASE_URL` | required by `exemplum-db` | Migration connection string (migrator credentials) |
| `EXEMPLUM_USERS_DATABASE_URL` | - | Runtime database connection string for the Users gRPC service |
| `EXEMPLUM_LOG` | `info` | Log level filter |
| `EXEMPLUM_LOG_FORMAT` | `text` | Logging format (`text` or `json`) |
| `EXEMPLUM_ENV_FILE` | unset | Dotenv file path selector (`""` to disable file loading) |

Docker Compose reads these (all optional, with development defaults):

| Variable | Default | Purpose |
| --- | --- | --- |
| `EXEMPLUM_POSTGRES_PORT` | `5432` | PostgreSQL host port |
| `EXEMPLUM_POSTGRES_PASSWORD`, `EXEMPLUM_MIGRATION_PASSWORD`, `EXEMPLUM_USERS_PASSWORD`, `EXEMPLUM_TASKS_PASSWORD` | `password` | Role passwords set by `make db-provision` |
| `EXEMPLUM_KRATOS_DB_PASSWORD` | `password` | Password of the `kratos` database role |
| `EXEMPLUM_KRATOS_COOKIE_SECRET` | dev value | Kratos cookie secret (at least 16 characters) |
| `EXEMPLUM_KRATOS_CIPHER_SECRET` | dev value | Kratos cipher secret (exactly 32 characters) |
| `EXEMPLUM_KRATOS_PORT` | `4433` | Kratos public API host port |
| `EXEMPLUM_CADDY_PORT` | `8080` | Browser origin port (see Known limits) |
| `EXEMPLUM_CADDY_APP_PORT` | `3000` | Host port Caddy proxies to; match `EXEMPLUM_APP_BIND` |
| `EXEMPLUM_KRATOS_CONFIG_DIR` | `./infra/kratos` | Kratos config directory mounted into the containers |

## Development Commands

All tasks can be executed using `make` from within the `example/` directory, or from the root via `make -C example <task>`.

### Database Lifecycle

Database operations are executed by `exemplum-db`, which constructs an application-specific `DatabaseConfig` (defining roles `exemplum_migrator`, `exemplum_owner`, `exemplum_tasks`, schema `exemplum`, registered domain migrators, and service contracts) and drives the decoupled framework migration engine `hypgmx-rdbms`.

```sh
make db-up        # Start PostgreSQL container in background
make db-status    # Inspect container status
make db-provision # Create roles and databases (including the kratos role and database)
make db-migrate   # Apply migrations via exemplum-db, then Kratos migrations (one-shot container)
make db-verify    # Verify database schema contracts
make db-down      # Stop all containers (PostgreSQL, Kratos, Caddy)
```

`exemplum-db inspect --output json` reports each schema's contracts as an array
under `data.contracts.<schema>`, preserving all tables registered in that schema.
Kratos migrates its own database only through `make db-migrate`; `kratos serve`
never migrates.

### Running the Application

From `example/`, with Docker running:

```sh
cp env/local.env.example env/local.env   # required: exemplum-db and the app read their database URLs from it
make db-up
make db-provision
make db-migrate
make app          # starts Kratos and Caddy (auth-up), then the app on 127.0.0.1:3000
```

Browse **http://localhost:8080**. The app checks database readiness and Kratos
`/health/ready` before binding its listener, and never runs migrations.

#### Bootstrap an administrator

No account is seeded. Register in the UI (http://localhost:8080/registration);
you land on the pending page, which also creates the account row. Then:

```sh
EXEMPLUM_ADMIN_DATABASE_URL=postgres://exemplum_users:password@127.0.0.1:5432/exemplum \
  cargo run --locked -p exemplum-admin-cli --bin exemplum-admin -- promote <username>
```

`promote` approves the account and makes it an administrator, printing one JSON
object. It uses the restricted users role, never the migration account, and
fails if nobody with that username has signed in yet. Administrators approve
further registrations at `/admin/users` (filter with `?status=pending|approved`,
page with `?after=`) or through `GET /api/admin/users` and
`POST /api/admin/users/{id}/approve`. Approval is idempotent, and both HTML and
JSON writes reject cross-site fetch metadata or a foreign Origin.

Verify the running application:

```sh
curl http://localhost:8080/healthz
curl -X POST http://localhost:8080/api/hello -H "Content-Type: application/json" -d '{"name":"Ada"}'
curl -i http://localhost:8080/api/hello/me   # 401 without a session
```

The greeting page at `/` works without JavaScript and is enhanced with the
vendored HTMX served from `/assets`. Every path under `/api` keeps the JSON error
envelope; page routes return safe HTML errors. See
[HTML Views and HTMX](../docs/app.md#7-html-views-and-htmx).

### Tests

`make test` runs every suite with its own Testcontainers (Docker must be
running); `cmd/app/tests/runtime.rs` includes an end-to-end run of the app
against a real Kratos container (register, pending `401`, promote `200`, logout `401`).

### Browser Smoke Checks

`make test-browser` (Node.js and Docker) runs Playwright view checks
(`make test-browser-views`) and a registration/login/logout run through Caddy
(`make test-browser-auth`) on an isolated stack with free ports and a scratch
Kratos config, set up and torn down by `tests/browser/auth-stack.mjs`.

### Verification & CI

```sh
make fmt          # Verify source code formatting
make fmt-fix      # Automatically format source code
make clippy       # Run clippy with -D warnings
make build        # Build all workspace packages (not part of ci; test builds everything)
make test         # Run unit and integration tests (Docker must be running)
make sweep        # Remove target/ artifacts unused for 7 days (needs cargo-sweep)
make ci           # Run full CI suite (fmt, clippy, test, sweep)
```

The workspace keeps builds fast and `target/` bounded:

- `[profile.dev]` in `Cargo.toml` keeps line tables for workspace crates and
  drops debug info from dependencies; `strip = "none"` keeps proc-macro dylibs
  loadable on macOS.
- `.cargo/config.toml` disables incremental compilation, whose data is never
  pruned and which sccache cannot cache. Edit-rebuilds of one large crate are
  slower in exchange for bounded disk use and compiler-cache hits.
- When `CI` is set (other than `false` or `0`), `make` also sets `CARGO_INCREMENTAL=0`,
  `CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_DEV_STRIP=none`.
- Every CI cargo invocation uses `--locked --workspace --all-features` so
  artifacts are reused rather than rebuilt for a different feature set.
- `make sweep` runs at the end of `make ci` and is skipped when
  [cargo-sweep](https://github.com/holmgr/cargo-sweep) is not installed or
  `SKIP_SWEEP` is set.

When using [sccache](https://github.com/mozilla/sccache)
(`RUSTC_WRAPPER=sccache`), raise its 10 GiB default cap with
`SCCACHE_CACHE_SIZE`, set `SCCACHE_BASEDIRS` so separate checkouts or worktrees
share entries (they are keyed on absolute paths), and set
`SCCACHE_IDLE_TIMEOUT=0` so `sccache --show-stats` stays meaningful.

## Template Customization

Exemplum serves as the target template for new project generation. The repository-level script `scripts/rename-project.py` is scoped to inspect and customize files within `example/`, replacing `exemplum` / `Exemplum` identities with your project's chosen identity.
