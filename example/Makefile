# Exemplum development and CI tasks. Run `make help` to list targets.
.DEFAULT_GOAL := help
# `ci` relies on prerequisites running in order, even under -j.
.NOTPARALLEL:
.PHONY: help fmt fmt-fix clippy build test test-browser test-browser-registration sweep ci ci-fmt-fix \
	db-up db-down db-status db-provision db-migrate db-verify app

# Incremental data and debug info do not help a full CI build. STRIP=none
# keeps proc-macro dylibs loadable on macOS once debug info is off.
ifneq ($(filter-out false 0,$(CI)),)
export CARGO_INCREMENTAL := 0
export CARGO_PROFILE_DEV_DEBUG := 0
export CARGO_PROFILE_DEV_STRIP := none
endif

# docker compose with env/local.env when present. EXEMPLUM_ENV_FILE selects
# another env file; setting it empty uses none.
COMPOSE = set --; \
	if [ -n "$${EXEMPLUM_ENV_FILE+x}" ]; then [ -z "$$EXEMPLUM_ENV_FILE" ] || set -- --env-file "$$EXEMPLUM_ENV_FILE"; \
	elif [ -f env/local.env ]; then set -- --env-file env/local.env; fi; \
	docker compose "$$@"

BROWSER_DEPS = cd tests/browser && npm ci && npx playwright install chromium

help: ## List targets
	@grep -E '^[a-z-]+:.*## ' $(firstword $(MAKEFILE_LIST)) | awk -F ':.*## ' '{printf "  %-26s %s\n", $$1, $$2}'

fmt: ## Check formatting
	cargo fmt --all -- --check

fmt-fix: ## Format sources
	cargo fmt --all

clippy: ## Lint with -D warnings
	cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

build: ## Build all packages (not part of ci; test builds everything)
	cargo build --locked --workspace --all-features

test: ## Run all tests; Docker must be running for integration suites
	cargo test --locked --workspace --all-features

test-browser: ## Playwright smoke checks for HTML views, JavaScript on and off (needs Node.js)
	$(BROWSER_DEPS) && npm test

test-browser-registration: ## Real PostgreSQL/app registration flow in Chromium, JavaScript on and off
	($(BROWSER_DEPS)) && EXEMPLUM_TEST_BROWSER=1 cargo test --locked --workspace --all-features --test runtime

sweep: ## Remove target/ artifacts unused for 7 days (needs cargo-sweep; skipped if SKIP_SWEEP is set)
ifdef SKIP_SWEEP
	@echo "SKIP_SWEEP set; skipping sweep"
else
	@if cargo sweep --version >/dev/null 2>&1; then cargo sweep --time 7; else echo "cargo-sweep not installed; skipping sweep"; fi
endif

ci: fmt clippy test sweep ## Full CI: fmt, clippy, test, sweep

ci-fmt-fix: fmt-fix clippy test sweep ## Like ci, but formats instead of checking

db-up: ## Start the PostgreSQL container in the background
	@$(COMPOSE) up --detach --wait

db-down: ## Stop the PostgreSQL container
	@$(COMPOSE) down

db-status: ## Show PostgreSQL container status
	@$(COMPOSE) ps

db-provision: ## Create roles, database, and schema contracts
	@$(COMPOSE) exec -T postgres psql -X -U admin -d exemplum -f /provision/provision.sql

db-migrate: ## Apply database migrations via exemplum-db
	cargo run --locked -p exemplum-db-cli --bin exemplum-db -- migrate

db-verify: ## Verify database schema contracts
	cargo run --locked -p exemplum-db-cli --bin exemplum-db -- verify

app: ## Run the public HTTP application against provisioned PostgreSQL
	cargo run --locked -p exemplum-app-cli --bin app
