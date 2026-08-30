.PHONY: help makehelp dev web-dev web-next-dev api-dev server rust-server daemon cli patchbay rust-cli build-rust-cli build rust-build test rust-test migrate-up migrate-down rust-migrate-up rust-migrate-down seed clean setup start stop check worktree-env setup-main start-main stop-main check-main setup-worktree start-worktree stop-worktree check-worktree remove-worktree db-up db-down db-drop db-reset selfhost selfhost-build selfhost-stop

MAIN_ENV_FILE ?= .env
WORKTREE_ENV_FILE ?= .env.worktree
ENV_FILE ?= $(if $(wildcard $(MAIN_ENV_FILE)),$(MAIN_ENV_FILE),$(if $(wildcard $(WORKTREE_ENV_FILE)),$(WORKTREE_ENV_FILE),$(MAIN_ENV_FILE)))

ifneq ($(wildcard $(ENV_FILE)),)
include $(ENV_FILE)
endif

POSTGRES_DB ?= patchbay
POSTGRES_USER ?= patchbay
POSTGRES_PASSWORD ?= patchbay
POSTGRES_PORT ?= 5432
PORT := $(or $(BACKEND_PORT),$(API_PORT),$(SERVER_PORT),$(PORT),8080)
ifeq ($(origin PATCHBAY_PUBLIC_URL), undefined)
PATCHBAY_PUBLIC_URL := http://localhost:$(PORT)
endif
FRONTEND_PORT ?= 3000
FRONTEND_ORIGIN ?= http://localhost:$(FRONTEND_PORT)
PATCHBAY_APP_URL ?= $(FRONTEND_ORIGIN)
DATABASE_URL ?= postgres://$(POSTGRES_USER):$(POSTGRES_PASSWORD)@localhost:$(POSTGRES_PORT)/$(POSTGRES_DB)?sslmode=disable
NEXT_PUBLIC_API_URL ?= http://localhost:$(PORT)
NEXT_PUBLIC_WS_URL ?= ws://localhost:$(PORT)/ws
# The shared Desktop renderer is also served by Vite. Keep its public
# endpoint variables aligned with the worktree values used by the Next.js
# client, so `make web-dev` cannot silently fall back to localhost:8080.
ifeq ($(strip $(VITE_API_URL)),)
VITE_API_URL := $(NEXT_PUBLIC_API_URL)
endif
ifeq ($(strip $(VITE_WS_URL)),)
VITE_WS_URL := $(NEXT_PUBLIC_WS_URL)
endif
GOOGLE_REDIRECT_URI ?= $(FRONTEND_ORIGIN)/auth/callback
PATCHBAY_SERVER_URL ?= ws://localhost:$(PORT)/ws
LOCAL_UPLOAD_BASE_URL ?= http://localhost:$(PORT)

export

PATCHBAY_ARGS ?= $(ARGS)

COMPOSE := docker compose

define REQUIRE_ENV
	@if [ ! -f "$(ENV_FILE)" ]; then \
		echo "Missing env file: $(ENV_FILE)"; \
		echo "Create .env from .env.example, or run 'make worktree-env' and use .env.worktree."; \
		exit 1; \
	fi
endef

# The Rust workspace is the source/runtime entrypoint. The wrapper keeps
# Cargo's workspace working directory during local development.
RUST_RUNNER := ./scripts/run-rust.sh
RUST_SERVER_CMD = PATCHBAY_BUILD_VERSION="$(VERSION)" PATCHBAY_BUILD_COMMIT="$(COMMIT)" PATCHBAY_BUILD_DATE="$(RUST_BUILD_DATE)" PATCHBAY_GIT_COMMIT="$(COMMIT)" PATCHBAY_SHUTDOWN_HOLD_DURATION="$(PATCHBAY_SHUTDOWN_HOLD_DURATION)" $(RUST_RUNNER) run --locked -p patchbay-server
RUST_MIGRATE_CMD = $(RUST_RUNNER) run --locked -p patchbay-migrate --

# Self-hosting requires the Docker Compose CLI plugin (`docker compose`).
# The self-host compose files use compose-spec syntax (top-level `name:`, no
# `version:`) that the legacy v1 `docker-compose` standalone cannot parse, so we
# fail early with an actionable message instead of a cryptic CLI parse error
# (e.g. "unknown shorthand flag: 'f' in -f") when the plugin is missing or v1.
# Keep the message short and OS-agnostic: per-OS install steps belong in docs.
define REQUIRE_COMPOSE
	@if ! compose_version=$$($(COMPOSE) version --short 2>/dev/null); then \
		echo "Docker Compose ('docker compose') was not found."; \
		echo "Self-hosting requires the Compose CLI plugin; legacy 'docker-compose' v1 is not supported."; \
		echo "Install Docker Compose from https://docs.docker.com/compose/install/ and verify with: docker compose version"; \
		exit 1; \
	fi; \
	case "$$compose_version" in \
		1.*|v1.*) \
			echo "'$(COMPOSE)' is legacy Docker Compose v1 ($$compose_version)."; \
			echo "Self-hosting requires the Compose CLI plugin; legacy 'docker-compose' v1 is not supported."; \
			echo "Install Docker Compose from https://docs.docker.com/compose/install/ and verify with: docker compose version"; \
			exit 1; \
			;; \
	esac
endef

# Default target changed from selfhost to help: bare `make` now prints this help
# instead of launching a full Docker Compose build, which is safer for onboarding.
.DEFAULT_GOAL := help

##@ Help

help: ## Show available make targets and common local workflows
	@awk 'BEGIN {FS = ":.*## "; printf "\nUsage:\n  make \033[36m<target>\033[0m\n\nQuick start:\n  \033[36mmake dev\033[0m          Bootstrap the current checkout and start everything\n  \033[36mmake check\033[0m        Run the full local verification pipeline\n\nCheckout modes:\n  Main checkout uses \033[36m.env\033[0m\n  Worktrees use \033[36m.env.worktree\033[0m (generate with \033[36mmake worktree-env\033[0m)\n\n"} \
		/^##@/ {printf "\n\033[1m%s\033[0m\n", substr($$0, 5); next} \
		/^[a-zA-Z0-9_.-]+:.*## / {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

makehelp: help ## Alias for `make help`

# ---------- Self-hosting (Docker Compose) ----------
##@ Self-hosting

selfhost: ## Create .env if needed, then pull and start the official self-hosted images
	$(REQUIRE_COMPOSE)
	@if [ ! -f .env ]; then \
		echo "==> Creating .env from .env.example..."; \
		cp .env.example .env; \
		JWT=$$(openssl rand -hex 32); \
		PGPASS=$$(openssl rand -hex 24); \
		VCSKEY=$$(openssl rand -base64 32); \
		if [ "$$(uname)" = "Darwin" ]; then \
			sed -i '' "s/^JWT_SECRET=.*/JWT_SECRET=$$JWT/" .env; \
			sed -i '' "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$$PGPASS/" .env; \
			sed -i '' -E "s#^(DATABASE_URL=postgres://[^:]+:)[^@]*(@.*)#\1$$PGPASS\2#" .env; \
			sed -i '' "s#^PATCHBAY_VCS_SECRET_KEY=.*#PATCHBAY_VCS_SECRET_KEY=$$VCSKEY#" .env; \
		else \
			sed -i "s/^JWT_SECRET=.*/JWT_SECRET=$$JWT/" .env; \
			sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$$PGPASS/" .env; \
			sed -i -E "s#^(DATABASE_URL=postgres://[^:]+:)[^@]*(@.*)#\1$$PGPASS\2#" .env; \
			sed -i "s#^PATCHBAY_VCS_SECRET_KEY=.*#PATCHBAY_VCS_SECRET_KEY=$$VCSKEY#" .env; \
		fi; \
		echo "==> Generated random JWT_SECRET, POSTGRES_PASSWORD, and PATCHBAY_VCS_SECRET_KEY"; \
	fi
	@echo "==> Pulling official Patchbay images..."
	@if ! $(COMPOSE) -f docker-compose.selfhost.yml pull; then \
		echo ""; \
		echo "Official images for tag '$${PATCHBAY_IMAGE_TAG:-latest}' are not published yet."; \
		echo "If this is before the first GHCR release, build from the current checkout:"; \
		echo "  make selfhost-build"; \
		exit 1; \
	fi
	@echo "==> Starting Patchbay via Docker Compose..."
	$(COMPOSE) -f docker-compose.selfhost.yml up -d
	@bash scripts/selfhost-wait.sh official

selfhost-build: ## Build backend/web from the current checkout and start the self-hosted stack
	$(REQUIRE_COMPOSE)
	@if [ ! -f .env ]; then \
		echo "==> Creating .env from .env.example..."; \
		cp .env.example .env; \
		JWT=$$(openssl rand -hex 32); \
		PGPASS=$$(openssl rand -hex 24); \
		VCSKEY=$$(openssl rand -base64 32); \
		if [ "$$(uname)" = "Darwin" ]; then \
			sed -i '' "s/^JWT_SECRET=.*/JWT_SECRET=$$JWT/" .env; \
			sed -i '' "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$$PGPASS/" .env; \
			sed -i '' -E "s#^(DATABASE_URL=postgres://[^:]+:)[^@]*(@.*)#\1$$PGPASS\2#" .env; \
			sed -i '' "s#^PATCHBAY_VCS_SECRET_KEY=.*#PATCHBAY_VCS_SECRET_KEY=$$VCSKEY#" .env; \
		else \
			sed -i "s/^JWT_SECRET=.*/JWT_SECRET=$$JWT/" .env; \
			sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$$PGPASS/" .env; \
			sed -i -E "s#^(DATABASE_URL=postgres://[^:]+:)[^@]*(@.*)#\1$$PGPASS\2#" .env; \
			sed -i "s#^PATCHBAY_VCS_SECRET_KEY=.*#PATCHBAY_VCS_SECRET_KEY=$$VCSKEY#" .env; \
		fi; \
		echo "==> Generated random JWT_SECRET, POSTGRES_PASSWORD, and PATCHBAY_VCS_SECRET_KEY"; \
	fi
	@echo "==> Building Patchbay from the current checkout..."
	$(COMPOSE) -f docker-compose.selfhost.yml -f docker-compose.selfhost.build.yml up -d --build
	@bash scripts/selfhost-wait.sh build

selfhost-stop: ## Stop the self-hosted Docker Compose stack
	$(REQUIRE_COMPOSE)
	@echo "==> Stopping Patchbay services..."
	$(COMPOSE) -f docker-compose.selfhost.yml down
	@echo "✓ All services stopped."

# ---------- One-click commands ----------
##@ One-click

setup: ## Prepare the current checkout from its env file: install deps, ensure DB, run migrations
	$(REQUIRE_ENV)
	@echo "==> Using env file: $(ENV_FILE)"
	@echo "==> Installing dependencies..."
	pnpm install
	@bash scripts/ensure-postgres.sh "$(ENV_FILE)"
	@echo "==> Running migrations..."
	$(RUST_MIGRATE_CMD) up
	@echo ""
	@echo "✓ Setup complete! Run 'make start' to launch the app."

start: ## Start backend and frontend for the current checkout and run migrations first
	$(REQUIRE_ENV)
	@echo "Using env file: $(ENV_FILE)"
	@echo "Backend: http://localhost:$(PORT)"
	@echo "Frontend: http://localhost:$(FRONTEND_PORT)"
	@bash scripts/ensure-postgres.sh "$(ENV_FILE)"
	@echo "Running migrations..."
	$(RUST_MIGRATE_CMD) up
	@echo "Starting backend and frontend..."
	@trap 'kill 0' EXIT; \
		($(RUST_SERVER_CMD)) & \
		pnpm dev:web & \
		wait

stop: ## Stop backend and frontend processes for the current checkout
	$(REQUIRE_ENV)
	@echo "Stopping services..."
	@-lsof -ti:$(PORT) | xargs kill -9 2>/dev/null
	@-lsof -ti:$(FRONTEND_PORT) | xargs kill -9 2>/dev/null
	@case "$(DATABASE_URL)" in \
		""|*@localhost:*|*@localhost/*|*@127.0.0.1:*|*@127.0.0.1/*|*@\[::1\]:*|*@\[::1\]/*) \
			echo "✓ App processes stopped. Shared PostgreSQL is still running on localhost:$(POSTGRES_PORT)." ;; \
		*) \
			echo "✓ App processes stopped. Remote PostgreSQL was not affected." ;; \
	esac

check: ## Run typecheck, TS tests, Rust tests, a Rust build, and Playwright E2E
	$(REQUIRE_ENV)
	@ENV_FILE="$(ENV_FILE)" bash scripts/check.sh

db-up: ## Start the shared PostgreSQL container used by main and worktrees
	@$(COMPOSE) up -d postgres

db-down: ## Stop the shared PostgreSQL container without removing its Docker volume
	@$(COMPOSE) down

db-drop: ## Permanently drop the current env's local database after confirmation
	$(REQUIRE_ENV)
	@status=0; bash scripts/drop-database.sh "$(ENV_FILE)" || status=$$?; \
		if [ "$$status" -eq 2 ]; then exit 0; fi; \
		exit "$$status"

# Drop + recreate the current env's database, then run all migrations.
# Use for a clean slate in local dev. Only affects the DB named in
# ENV_FILE (POSTGRES_DB); the shared postgres container and other
# worktree DBs are untouched. Refuses to run against a remote host.
db-reset: ## Drop and recreate the current env's database, then re-run all migrations
	$(REQUIRE_ENV)
	@case "$(DATABASE_URL)" in \
		""|*@localhost:*|*@localhost/*|*@127.0.0.1:*|*@127.0.0.1/*|*@\[::1\]:*|*@\[::1\]/*) ;; \
		*) echo "Refusing to reset: DATABASE_URL points at a remote host."; exit 1 ;; \
	esac
	@bash scripts/ensure-postgres.sh "$(ENV_FILE)"
	@echo "==> Dropping and recreating database '$(POSTGRES_DB)'..."
	@$(COMPOSE) exec -T postgres psql -U $(POSTGRES_USER) -d postgres -v ON_ERROR_STOP=1 \
		-c "DROP DATABASE IF EXISTS \"$(POSTGRES_DB)\" WITH (FORCE);" \
		-c "CREATE DATABASE \"$(POSTGRES_DB)\";"
	@echo "==> Running migrations..."
	$(RUST_MIGRATE_CMD) up
	@echo ""
	@echo "✓ Database '$(POSTGRES_DB)' reset. Run 'make start' to launch the app."

worktree-env: ## Generate .env.worktree with a unique DB name and app ports for this worktree
	@bash scripts/init-worktree-env.sh .env.worktree

setup-main: ## Prepare the main checkout using .env
	@$(MAKE) setup ENV_FILE=$(MAIN_ENV_FILE)

start-main: ## Start the main checkout using .env
	@$(MAKE) start ENV_FILE=$(MAIN_ENV_FILE)

stop-main: ## Stop the main checkout processes defined by .env
	@$(MAKE) stop ENV_FILE=$(MAIN_ENV_FILE)

check-main: ## Run the full verification pipeline for the main checkout
	@ENV_FILE=$(MAIN_ENV_FILE) bash scripts/check.sh

setup-worktree: ## Ensure .env.worktree exists, then prepare this worktree
	@if [ ! -f "$(WORKTREE_ENV_FILE)" ]; then \
		echo "==> Generating $(WORKTREE_ENV_FILE) with unique ports..."; \
		bash scripts/init-worktree-env.sh $(WORKTREE_ENV_FILE); \
	else \
		echo "==> Using existing $(WORKTREE_ENV_FILE)"; \
	fi
	@$(MAKE) setup ENV_FILE=$(WORKTREE_ENV_FILE)

start-worktree: ## Start this worktree using .env.worktree
	@$(MAKE) start ENV_FILE=$(WORKTREE_ENV_FILE)

stop-worktree: ## Stop this worktree's backend and frontend processes
	@$(MAKE) stop ENV_FILE=$(WORKTREE_ENV_FILE)

check-worktree: ## Run the full verification pipeline for this worktree
	@ENV_FILE=$(WORKTREE_ENV_FILE) bash scripts/check.sh

remove-worktree: ## Drop a linked worktree's database, then remove it (WORKTREE=path)
	@bash scripts/remove-worktree.sh "$(WORKTREE)"

# ---------- Individual commands ----------
##@ Individual commands

dev: ## Bootstrap this checkout end-to-end: create env if needed, ensure DB, migrate, start services
	@bash scripts/dev.sh

web-dev: ## Run the Desktop renderer in a browser through Vite (API-dependent screens need a separate backend)
	@echo "Frontend: http://localhost:$(FRONTEND_PORT)"
	@FRONTEND_PORT=$(FRONTEND_PORT) pnpm dev:web

web-next-dev: ## Run only the Next.js web frontend (API-dependent screens need a separate backend)
	@echo "Frontend: http://localhost:$(FRONTEND_PORT)"
	@pnpm dev:web:next

api-dev: ## Run only the API/WebSocket backend (PostgreSQL must already be running)
	$(REQUIRE_ENV)
	@echo "Backend: http://localhost:$(PORT)"
	$(RUST_SERVER_CMD)

daemon: PATCHBAY_ARGS := daemon restart --profile local
daemon: rust-cli ## Restart the local agent daemon using the CLI's stored auth/session

server: ## Run only the Rust server for the current checkout
	$(REQUIRE_ENV)
	@bash scripts/ensure-postgres.sh "$(ENV_FILE)"
	$(RUST_SERVER_CMD)

rust-server: server ## Run the migrated Rust server entrypoint

cli: rust-cli ## Run the Rust patchbay CLI with ARGS or PATCHBAY_ARGS from source

patchbay: rust-cli ## Run the Rust patchbay CLI entrypoint

rust-cli: ## Run the migrated Rust CLI slice with ARGS or PATCHBAY_ARGS
	PATCHBAY_BUILD_VERSION="$(VERSION)" PATCHBAY_BUILD_COMMIT="$(COMMIT)" PATCHBAY_BUILD_DATE="$(RUST_BUILD_DATE)" $(RUST_RUNNER) run --locked -p patchbay-cli -- $(PATCHBAY_ARGS)

build-rust-cli: ## Build the migrated Rust CLI slice in release mode
	CARGO_TARGET_DIR="$(RUST_TARGET_DIR)" PATCHBAY_BUILD_VERSION="$(VERSION)" PATCHBAY_BUILD_COMMIT="$(COMMIT)" PATCHBAY_BUILD_DATE="$(DATE)" $(RUST_RUNNER) build --release --locked -p patchbay-cli

VERSION ?= $(shell git describe --tags --match 'v[0-9]*' --always --dirty 2>/dev/null || echo dev)
COMMIT  ?= $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)
DATE    ?= $(shell date -u '+%Y-%m-%dT%H:%M:%SZ')

RUST_BUILD_DATE ?= $(shell git show -s --format=%cI HEAD 2>/dev/null || echo unknown)
RUST_TARGET_DIR ?= $(CURDIR)/server-rs/target
RUST_EXE ?= $(if $(filter Windows_NT,$(OS)),.exe,)

build: rust-build ## Build Rust server, CLI, migration, and backfill binaries into bin

rust-build: ## Build native Rust server, CLI, migration, and backfill binaries into bin
	@mkdir -p bin
	CARGO_TARGET_DIR="$(RUST_TARGET_DIR)" PATCHBAY_BUILD_VERSION="$(VERSION)" PATCHBAY_BUILD_COMMIT="$(COMMIT)" PATCHBAY_BUILD_DATE="$(DATE)" PATCHBAY_GIT_COMMIT="$(COMMIT)" $(RUST_RUNNER) build --release --locked -p patchbay-server -p patchbay-cli -p patchbay-migrate --bins
	cp "$(RUST_TARGET_DIR)/release/patchbay-server$(RUST_EXE)" "bin/server$(RUST_EXE)"
	cp "$(RUST_TARGET_DIR)/release/patchbay$(RUST_EXE)" "bin/patchbay$(RUST_EXE)"
	cp "$(RUST_TARGET_DIR)/release/patchbay-migrate$(RUST_EXE)" "bin/migrate$(RUST_EXE)"
	cp "$(RUST_TARGET_DIR)/release/backfill_task_usage_hourly$(RUST_EXE)" "bin/backfill_task_usage_hourly$(RUST_EXE)"
	cp "$(RUST_TARGET_DIR)/release/backfill_issue_last_activity$(RUST_EXE)" "bin/backfill_issue_last_activity$(RUST_EXE)"
	cp "$(RUST_TARGET_DIR)/release/backfill_codex_usage_cache$(RUST_EXE)" "bin/backfill_codex_usage_cache$(RUST_EXE)"

test: rust-test ## Run Rust tests after ensuring the target DB exists and migrations are applied

rust-test: ## Run Rust workspace tests after ensuring the target DB exists and migrations are applied
	$(REQUIRE_ENV)
	@bash scripts/ensure-postgres.sh "$(ENV_FILE)"
	$(RUST_MIGRATE_CMD) up
	$(RUST_RUNNER) test --workspace --all-targets --locked

# Database
##@ Database

migrate-up: rust-migrate-up ## Create the target DB if needed, then apply database migrations

rust-migrate-up: ## Apply database migrations with the Rust runner
	$(REQUIRE_ENV)
	@bash scripts/ensure-postgres.sh "$(ENV_FILE)"
	$(RUST_MIGRATE_CMD) up

migrate-down: rust-migrate-down ## Create the target DB if needed, then roll back database migrations

rust-migrate-down: ## Roll back database migrations with the Rust runner
	$(REQUIRE_ENV)
	@bash scripts/ensure-postgres.sh "$(ENV_FILE)"
	$(RUST_MIGRATE_CMD) down

# Cleanup
##@ Cleanup

clean: ## Remove build caches, generated binaries, and temp files
	rm -rf bin
	rm -rf apps/*/.next apps/*/.source apps/*/.expo
	rm -rf apps/*/out apps/*/dist apps/*/dist-electron packages/*/dist
	rm -rf .turbo apps/*/.turbo packages/*/.turbo
	rm -rf apps/*/*.tsbuildinfo packages/*/*.tsbuildinfo
	@echo "✓ Clean complete."
