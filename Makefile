# Framework development and CI tasks. Run `make help` to list targets.
.DEFAULT_GOAL := help
# `ci` relies on prerequisites running in order, even under -j.
.NOTPARALLEL:
.PHONY: help fmt fmt-fix clippy build test test-rename example-ci sweep ci ci-fmt-fix apalis-dashboard

# Incremental data and debug info do not help a full CI build. STRIP=none
# keeps proc-macro dylibs loadable on macOS once debug info is off.
ifneq ($(filter-out false 0,$(CI)),)
export CARGO_INCREMENTAL := 0
export CARGO_PROFILE_DEV_DEBUG := 0
export CARGO_PROFILE_DEV_STRIP := none
endif

help: ## List targets
	@grep -E '^[a-z-]+:.*## ' $(firstword $(MAKEFILE_LIST)) | awk -F ':.*## ' '{printf "  %-18s %s\n", $$1, $$2}'

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

test-rename: ## Test project customization in disposable Git repositories
	python3 -m unittest discover -s scripts/tests -v

# The example shares this target dir, so the root sweep covers it.
example-ci: ## Run CI across the Exemplum reference project
	$(MAKE) -C example ci SKIP_SWEEP=1

sweep: ## Remove target/ artifacts unused for 7 days (needs cargo-sweep)
	@if cargo sweep --version >/dev/null 2>&1; then cargo sweep --time 7; else echo "cargo-sweep not installed; skipping sweep"; fi

ci: fmt clippy test test-rename example-ci sweep ## Full CI: fmt, clippy, test, test-rename, example CI, sweep

ci-fmt-fix: fmt-fix clippy test test-rename example-ci sweep ## Like ci, but formats instead of checking

apalis-dashboard: ## Run the standalone task dashboard
	cargo run --locked -p apalis-dashboard
