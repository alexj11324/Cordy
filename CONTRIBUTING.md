# Contributing Guide

This guide documents the local development workflow for contributors working on the Patchbay codebase.

It covers:

- first-time setup
- day-to-day development in the main checkout
- isolated worktree development
- the shared PostgreSQL model
- testing and verification
- full-stack isolated testing (backend + frontend + daemon from source)
- troubleshooting and destructive reset options

## Contribution Terms

By submitting a contribution to Patchbay — a pull request, a patch, or any
other work — you agree to condition 2 of the [Patchbay License](LICENSE):

- your contribution is submitted under the Patchbay License as a whole (the
  additional conditions in Part I together with the incorporated Apache
  License 2.0 text in Part II), not under the Apache License 2.0 alone;
- your contributed code may be used for commercial purposes, including the
  producer's cloud business operations;
- the producer can adjust the Patchbay License to be more strict or relaxed
  as deemed necessary.

See the [LICENSE](LICENSE) file for the full terms.

## Development Model

Local development uses one shared PostgreSQL service and one database per checkout.

- the main checkout usually uses `.env` and `POSTGRES_DB=patchbay`
- each Git worktree uses its own `.env.worktree`
- every checkout connects to the same PostgreSQL host: `localhost:5432`
- isolation happens at the database level, not by starting a separate PostgreSQL service
- backend and frontend ports are still unique per worktree

The service may be Docker Compose or a native local PostgreSQL installation;
schema and data remain isolated either way.
`PATCHBAY_POSTGRES_RUNTIME=auto` selects Docker only for the Compose-published
`localhost:5432` endpoint and uses the configured native endpoint otherwise.
Set it to `native` or `docker` when localhost:5432 is intentionally ambiguous.

## Prerequisites

- Node.js `22`
- `pnpm` `10.28.2`
- stable Rust toolchain with Cargo (required on a dev runtime cache miss)
- Docker, or native PostgreSQL 15+

`pnpm dev` is the cross-platform entrypoint. `make dev` is a POSIX convenience
alias and is not required on Windows.

## Important Rules

- The main checkout should use `.env`.
- A worktree should use `.env.worktree`.
- Do not copy `.env` into a worktree directory.

Why:

- the current command flow prefers `.env` over `.env.worktree`
- if a worktree contains `.env`, it can accidentally point back to the main database

## Environment Files

### Main Checkout

Create `.env` once:

```bash
cp .env.example .env
```

By default, `.env` points to:

```bash
POSTGRES_DB=patchbay
POSTGRES_PORT=5432
DATABASE_URL=postgres://patchbay:patchbay@localhost:5432/patchbay?sslmode=disable
PORT=8080
FRONTEND_PORT=3000
```

### Worktree

Generate `.env.worktree` from inside the worktree:

```bash
make worktree-env
```

That generates values like:

```bash
POSTGRES_DB=patchbay_my_feature_702
POSTGRES_PORT=5432
PORT=18782
FRONTEND_PORT=13702
DATABASE_URL=postgres://patchbay:patchbay@localhost:5432/patchbay_my_feature_702?sslmode=disable
```

Notes:

- `POSTGRES_DB` is unique per worktree
- `POSTGRES_PORT` stays fixed at `5432`
- backend and frontend ports are derived from the worktree path hash
- `make worktree-env` refuses to overwrite an existing `.env.worktree`

To regenerate a worktree env file:

```bash
FORCE=1 make worktree-env
```

## First-Time Setup

### Quick Start (recommended)

From any checkout (main or worktree):

```bash
make dev
```

This single command:

- auto-detects whether you're in a main checkout or a worktree
- creates the appropriate env file (`.env` or `.env.worktree`) if it doesn't exist
- checks the always-required prerequisites; Rust is required only on a runtime cache miss
- installs JavaScript dependencies
- uses the shared Docker or native PostgreSQL service
- creates the application database if it does not exist
- runs all migrations
- starts the backend and complete Electron client with renderer hot reload

### Explicit Setup (advanced)

If you prefer separate control over setup and startup:

#### Main Checkout

```bash
cp .env.example .env
make setup-main
make start-main
```

Stop:

```bash
make stop-main
```

#### Worktree

```bash
make worktree-env
make setup-worktree
make start-worktree
```

Stop:

```bash
make stop-worktree
```

## Recommended Daily Workflow

### Main Checkout

Use the main checkout when you want a stable local environment for `main`.

```bash
make start-main
make stop-main
make check-main
```

### Feature Worktree

Use a worktree when you want isolated data and separate app ports.

```bash
git worktree add ../patchbay-feature -b feat/my-change main
cd ../patchbay-feature
make dev
```

After that, day-to-day commands are:

```bash
make dev              # start (re-runs setup if needed, idempotent)
make stop-worktree    # stop
make check-worktree   # verify
```

### Removing a Worktree

Git does not provide a `pre-worktree-remove` hook. Use the repository wrapper
from another checkout so database cleanup happens before Git removes the
worktree directory:

```bash
make remove-worktree WORKTREE=../patchbay-feature
```

The command refuses to remove the primary checkout, the current checkout, a
locked worktree, or a worktree with uncommitted changes. If the target contains
`.env.worktree`, it shows the database name and asks for `y/N` confirmation,
drops that database, and only then runs `git worktree remove`. A worktree that
was never set up has no `.env.worktree`, so database cleanup is skipped.

Running `git worktree remove` directly bypasses this cleanup and can leave an
orphaned local database.

## Running Main and Worktree at the Same Time

This is a first-class workflow.

Example:

- main checkout
  - database: `patchbay`
  - backend: `8080`
  - frontend: `3000`
- worktree checkout
  - database: `patchbay_my_feature_702`
  - backend: generated worktree port such as `18782`
  - frontend: generated worktree port such as `13702`

Both checkouts use:

- the same PostgreSQL container
- the same PostgreSQL port: `5432`

But they do not share application data, because each uses a different database.

## Command Reference

### Shared Infrastructure

Start the shared PostgreSQL container:

```bash
make db-up
```

Stop the shared PostgreSQL container:

```bash
make db-down
```

Important:

- `make db-down` stops the container but keeps the Docker volume
- your local databases are preserved

### App Lifecycle

Main checkout:

```bash
make setup-main
make start-main
make stop-main
make check-main
```

Worktree:

```bash
make worktree-env
make setup-worktree
make start-worktree
make stop-worktree
make check-worktree
```

Generic targets for the current checkout:

```bash
make setup
make start
make stop
make check
make dev
make test
make migrate-up
make migrate-down
```

These generic targets require a valid env file in the current directory.

## How Database Creation Works

Database creation is automatic.

The following commands all ensure the target database exists before they continue:

- `make setup`
- `make start`
- `make dev`
- `make test`
- `make migrate-up`
- `make migrate-down`
- `make check`

That logic lives in `scripts/ensure-postgres.sh`.

## Testing

Run all local checks:

```bash
make check-main
```

Or from a worktree:

```bash
make check-worktree
```

This runs:

1. TypeScript typecheck
2. TypeScript unit tests
3. Rust workspace tests
4. Playwright E2E tests

Notes:

- Rust tests create their own fixture data
- E2E tests create their own workspace and issue fixtures
- the check flow starts backend/frontend only if they are not already running

## Local Codex Daemon

Run the local daemon:

```bash
make daemon
```

The daemon authenticates using the CLI's stored token (`patchbay login`).
It registers runtimes for all watched workspaces from the CLI config.

## Full-Stack Isolated Testing

This section covers running the complete stack (backend, frontend, daemon) from
source in a fully isolated environment. Useful for testing end-to-end changes
that span multiple components, or for automated CI/AI workflows that need zero
human intervention.

### Why Not Just `make daemon`?

`make daemon` uses the system-installed CLI's stored token and connects to
whatever server is configured in `~/.patchbay/config.json`. That's fine for
day-to-day development against a shared server, but for fully isolated testing
you need:

- a local backend and frontend (from source)
- a local daemon (from source) with its own profile
- automated authentication (no browser login)
- no interference with your production CLI config

### Dynamic Profile Naming

Each worktree must use a unique daemon profile to avoid collisions when
multiple features run in parallel.

The profile name is derived from the worktree directory using the same
slug + hash pattern as `scripts/init-worktree-env.sh`:

```bash
WORKTREE_DIR="$(basename "$PWD")"
SLUG="$(printf '%s' "$WORKTREE_DIR" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/_/g; s/__*/_/g; s/^_//; s/_$//')"
HASH="$(printf '%s' "$PWD" | cksum | awk '{print $1}')"
OFFSET=$((HASH % 1000))
PROFILE="dev-${SLUG}-${OFFSET}"
```

Example: worktree at `../patchbay-feat-auth` produces profile
`dev-patchbay_feat_auth-347`, matching that worktree's port and database
allocation.

### Start the Isolated Environment

Run all steps from the worktree root (where the Makefile is).

#### 1. Start backend, frontend, and database

```bash
make dev
```

Wait for the backend to be healthy:

```bash
PORT=$(grep '^PORT=' .env.worktree 2>/dev/null || grep '^PORT=' .env | head -1 | cut -d= -f2)
PORT=${PORT:-8080}
SERVER="http://localhost:${PORT}"

for i in $(seq 1 30); do
  curl -sf "$SERVER/health" > /dev/null 2>&1 && break
  sleep 2
done
```

#### 2. Create a test user and token (automated auth)

For deterministic local automation, set `PATCHBAY_DEV_VERIFICATION_CODE=888888`
in your env file before starting the backend:

```bash
curl -s -X POST "$SERVER/auth/send-code" \
  -H "Content-Type: application/json" \
  -d '{"email": "dev@localhost"}'

JWT=$(curl -s -X POST "$SERVER/auth/verify-code" \
  -H "Content-Type: application/json" \
  -d '{"email": "dev@localhost", "code": "888888"}' | jq -r '.token')

PAT=$(curl -s -X POST "$SERVER/api/tokens" \
  -H "Authorization: Bearer $JWT" \
  -H "Content-Type: application/json" \
  -d '{"name": "auto-dev", "expires_in_days": 365}' | jq -r '.token')
```

#### 3. Create a workspace

```bash
WS=$(curl -s -X POST "$SERVER/api/workspaces" \
  -H "Authorization: Bearer $PAT" \
  -H "Content-Type: application/json" \
  -d '{"name": "Dev", "slug": "dev"}' | jq -r '.id')
```

#### 4. Compute profile name and write CLI config

```bash
# Compute profile (see Dynamic Profile Naming above)
WORKTREE_DIR="$(basename "$PWD")"
SLUG="$(printf '%s' "$WORKTREE_DIR" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/_/g; s/__*/_/g; s/^_//; s/_$//')"
HASH="$(printf '%s' "$PWD" | cksum | awk '{print $1}')"
OFFSET=$((HASH % 1000))
PROFILE="dev-${SLUG}-${OFFSET}"

FRONTEND_PORT=$(grep '^FRONTEND_PORT=' .env.worktree 2>/dev/null || grep '^FRONTEND_PORT=' .env | head -1 | cut -d= -f2)
FRONTEND_PORT=${FRONTEND_PORT:-3000}

CONFIG_DIR="$HOME/.patchbay/profiles/$PROFILE"
mkdir -p "$CONFIG_DIR"

cat > "$CONFIG_DIR/config.json" << EOF
{
  "server_url": "$SERVER",
  "app_url": "http://localhost:${FRONTEND_PORT}",
  "token": "$PAT",
  "workspace_id": "$WS",
  "watched_workspaces": [{"id": "$WS", "name": "Dev"}]
}
EOF
```

#### 5. Start the daemon from source

```bash
make cli ARGS="daemon start --profile $PROFILE"
```

The daemon runs from the current worktree's Rust `patchbay-cli` package, connecting
to the local backend. Agent-executed `patchbay` commands automatically use the
same binary (the daemon prepends its own directory to `PATH`).

### Stop the Isolated Environment

```bash
# Compute profile (same formula)
PROFILE="dev-$(printf '%s' "$(basename "$PWD")" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/_/g; s/__*/_/g; s/^_//; s/_$//')-$(( $(printf '%s' "$PWD" | cksum | awk '{print $1}') % 1000 ))"

# 1. Stop daemon
make cli ARGS="daemon stop --profile $PROFILE"

# 2. Stop backend + frontend
make stop            # main checkout
make stop-worktree   # worktree checkout

# 3. (Optional) Stop shared PostgreSQL
make db-down

# 4. (Optional) Clean build artifacts
make clean

# 5. (Optional) Remove profile config
rm -rf "$HOME/.patchbay/profiles/$PROFILE"
```

### Desktop App Local Testing

Run the complete Electron development environment with one command:

```bash
pnpm dev
```

The command does not open Electron until it has:

1. Created or loaded the checkout-specific env, database, ports, and Electron
   `userData` identity
2. Installed dependencies using the global content-addressable pnpm store and
   this worktree's own `node_modules` link tree
3. Applied migrations and reached the local backend's DB-backed `/healthz`
   readiness endpoint
4. Staged checksum-valid dev CLI, backend, and migration binaries whose Rust
   source, Cargo manifests/lockfile, toolchain, target, architecture, profile,
   and build metadata match
5. Exercised local agent discovery through `patchbay daemon probe-runtimes`
   (the renderer never guesses the host PATH)
6. Verified Telegram and Weixin credential-encryption keys without logging
   their values

There is deliberately no UI-only or released/PATH-CLI development fallback. A
missing capability fails before the window opens and prints an executable fix.
Use `pnpm dev:doctor` to repeat the same diagnostics while the stack is running.

| Situation                                                                      | Command                                   | Expected work                                                                  |
| ------------------------------------------------------------------------------ | ----------------------------------------- | ------------------------------------------------------------------------------ |
| Normal local product development                                               | `pnpm dev` or `make dev`                  | Complete Electron + dev CLI + backend + isolated DB, with Vite hot reload      |
| Re-run capability diagnostics                                                  | `pnpm dev:doctor`                         | CLI/version/source, backend/DB, agent detection, Telegram/Weixin configuration |
| Compile-check frontend/Electron output                                         | `pnpm --filter @patchbay/desktop build`   | Electron/Vite production bundles; no Rust                                      |
| Validate an installer, signing/notarization, updater, embedded CLI, or release | `pnpm --filter @patchbay/desktop package` | Release Rust CLI and installer packaging; may take tens of minutes             |

The dev runtime cache is per user and content-addressed. Unchanged Rust source
reuses the CLI, backend, and migration runner without compiling, including in
another worktree; a miss builds all three together in the incremental dev
profile. Install `sccache` to share compiler outputs. Do not point multiple
worktrees at one `server-rs/target`: target directories, `node_modules` link
trees, databases, ports, env files, and processes stay isolated. Only pnpm's
store, Cargo registry/git downloads, sccache, and the verified dev runtime cache
are shared.

Do not use the package path for routine development. Release/package never
reads the dev cache: it performs the formal release build or consumes a
checksum-verified exact-commit CLI artifact produced by the release workflow.

CI follows the same boundary. It caches pnpm's store, Cargo registry/git
downloads, bounded sccache compiler objects, and Turbo outputs; it never uploads
an entire `server-rs/target`. Release CLIs/installers are exact-commit workflow
artifacts, not reusable caches. Cache keys remain OS/architecture/target aware,
and Rust jobs print sccache statistics so restore/upload time and hit rate can
be compared with cold compilation. Keep the repository within GitHub's default
cache budget unless measured savings justify a paid increase; shrinking or
removing a cache is correct when transfer time approaches rebuild time. See
[GitHub dependency caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)
and [repository Actions settings](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository).

Login in the Desktop UI with `dev@localhost` and the generated code from the
backend logs. If you set `PATCHBAY_DEV_VERIFICATION_CODE=888888` before starting
the backend, you can use `888888` instead.

Telegram still requires a real BotFather token entered through the integration
flow, and personal Weixin still requires a real iLink QR authorization. Without
those external credentials the UI must stop at the explicit provider step; a
configured encryption key is capability readiness, not proof of a successful
provider connection or test message.

#### Running multiple worktrees side-by-side

`pnpm dev` auto-isolates a worktree so several worktrees can run their
own desktop dev instance at once — no extra setup. From a linked worktree it
derives, from the worktree path (same `cksum % 1000` offset as the backend /
frontend ports in `.env.worktree`):

- `DESKTOP_RENDERER_PORT` = `5174 + offset` — its own Vite dev server (`5174`
  base leaves `5173` for the primary checkout, even when `offset` is `0`). The
  one offset that would land on `6000` gets `6174` instead: Chromium treats
  `6000` as a restricted port and fails the load with `ERR_UNSAFE_PORT`
- `DESKTOP_APP_SUFFIX` = `<folder>-<offset>` — its own single-instance lock /
  `userData`, and an app named `Patchbay Canary <folder>-<offset>` so it is
  distinguishable in Cmd+Tab. The offset keeps it unique across worktrees that
  share a folder name at different paths.

The primary checkout is left untouched (`5173`, `Patchbay Canary`). The complete
launcher exports each worktree's backend and WebSocket endpoints to Electron,
so no hand-written `apps/desktop/.env.development.local` is required.

### Isolation Guarantee

Nothing in this flow touches the system-installed `patchbay` or the default
`~/.patchbay/config.json`:

| Resource        | System / Production            | Local Dev (per-worktree)                             |
| --------------- | ------------------------------ | ---------------------------------------------------- |
| Config          | `~/.patchbay/config.json`      | `~/.patchbay/profiles/dev-<slug>-<hash>/config.json` |
| Daemon PID      | `~/.patchbay/daemon.pid`       | `~/.patchbay/profiles/dev-<slug>-<hash>/daemon.pid`  |
| Health port     | `19514`                        | `19514 + 1 + (name_hash % 1000)`                     |
| Workspaces dir  | `~/patchbay_workspaces/`       | `~/patchbay_workspaces_dev-<slug>-<hash>/`           |
| Database        | remote / production            | local Docker: `patchbay_<slug>_<hash>`               |
| Desktop profile | `desktop-api.aspectlylabs.com` | `desktop-localhost-<port>`                           |

Multiple worktrees can run simultaneously without conflict.

## Troubleshooting

### Missing Env File

If you see:

```text
Missing env file: .env
```

or:

```text
Missing env file: .env.worktree
```

then create the expected env file first.

Main checkout:

```bash
cp .env.example .env
```

Worktree:

```bash
make worktree-env
```

### Check Which Database a Checkout Uses

Inspect the env file:

```bash
cat .env
cat .env.worktree
```

Look for:

- `POSTGRES_DB`
- `DATABASE_URL`
- `PORT`
- `FRONTEND_PORT`

### List All Local Databases in Shared PostgreSQL

```bash
docker compose exec -T postgres psql -U patchbay -d postgres -At -c "select datname from pg_database order by datname;"
```

### Worktree Is Accidentally Using the Main Database

Check whether the worktree contains `.env`.

It should not.

The safe worktree setup is:

```bash
make worktree-env
make setup-worktree
make start-worktree
```

### App Stops but PostgreSQL Keeps Running

That is expected.

- `make stop`
- `make stop-main`
- `make stop-worktree`

only stop backend/frontend processes.

To stop the shared PostgreSQL container:

```bash
make db-down
```

## Destructive Reset

If you want to stop PostgreSQL and keep your local databases:

```bash
make db-down
```

If you want a fresh database for the current checkout only (drops the
database named in `POSTGRES_DB`, recreates it, and runs all migrations):

```bash
make stop        # stop backend/frontend first
make db-reset
make start
```

- only affects the current env's database; other worktree databases are untouched
- refuses to run if `DATABASE_URL` points at a remote host
- pass `ENV_FILE=.env.worktree` to target a specific worktree

To permanently drop the current worktree database without recreating it:

```bash
make db-drop ENV_FILE=.env.worktree
```

The command prints the selected database and environment file, then requires a
`y/N` confirmation. It only operates on the local Docker or native PostgreSQL service,
protects PostgreSQL system databases, and refuses to drop the default main
database `patchbay` unless `ALLOW_MAIN_DB_DROP=1` is explicitly supplied.
Declining the confirmation is a successful no-op; when called by
`make remove-worktree`, it also leaves the worktree in place.

If you want to wipe all local PostgreSQL data for this repo:

```bash
docker compose down -v
```

Warning:

- this deletes the shared Docker volume
- this deletes the main database and every worktree database in that volume
- after that you must run `make setup-main` or `make setup-worktree` again

## Typical Flows

### Stable Main Environment

```bash
make dev
```

### Feature Worktree

```bash
git worktree add ../patchbay-feature -b feat/my-change main
cd ../patchbay-feature
make dev
```

### Return to a Previously Configured Worktree

```bash
cd ../patchbay-feature
make start-worktree
```

### Validate Before Pushing

Main checkout:

```bash
make check-main
```

Worktree:

```bash
make check-worktree
```
