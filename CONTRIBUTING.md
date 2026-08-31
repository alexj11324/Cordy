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

Local development uses one shared PostgreSQL service and one database per
source-development checkout.

- the main checkout, independent clones, and Git worktrees each use their own `.env.worktree`
- `.env` is reserved for explicit non-development/self-host configuration
- every checkout connects to the same PostgreSQL host: `localhost:5432`
- isolation happens at the database level, not by starting a separate PostgreSQL service
- backend and frontend ports are unique per checkout

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

- Run `pnpm dev` (or `make dev`) from every source-development checkout.
- The complete launcher creates and validates that checkout's `.env.worktree`.
- Do not copy `.env` into a source-development checkout.
- If the launcher reports a stale generated environment, run `FORCE=1 make worktree-env` once and retry.
- Use `PATCHBAY_DEV_ENV_FILE=/absolute/path/to/file` only for an intentional,
  explicit runtime override.

## Environment Files

### Source-development checkout

The complete launcher creates `.env.worktree` automatically. To create it
before the first launch:

```bash
make worktree-env
```

Generated values are isolated to this checkout:

```bash
POSTGRES_DB=patchbay_my_feature_702
POSTGRES_PORT=5432
DATABASE_URL=postgres://patchbay:patchbay@localhost:5432/patchbay_my_feature_702?sslmode=disable
PORT=18782
FRONTEND_PORT=13702
```

`.env` remains available for explicit self-host or other non-development
commands; it is never selected implicitly by the complete Desktop launcher.

## First-Time Setup

### Quick Start (recommended)

From any checkout (main or worktree):

```bash
pnpm dev
```

This single command:

- creates and validates the checkout's isolated `.env.worktree`
- checks the always-required prerequisites; Rust is required only on a runtime cache miss
- installs JavaScript dependencies
- uses the shared Docker or native PostgreSQL service
- creates the application database if it does not exist
- runs all migrations
- starts the backend, browser login/share origin, and complete Electron client with renderer hot reload

`make dev` is a POSIX convenience alias for the same complete launcher. There
is intentionally no separate setup/start path: runtime, database, auth, and
capability checks must stay on one observable path.

## Recommended Daily Workflow

### Feature Worktree

Use a worktree when you want isolated data and separate app ports.

```bash
git worktree add ../patchbay-feature -b feat/my-change main
cd ../patchbay-feature
make dev
```

After that, day-to-day commands are:

```bash
pnpm dev              # complete Desktop stack; setup is idempotent
make stop             # stop this checkout's tracked process tree
make check            # explicit full local verification
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

## Running Multiple Checkouts at the Same Time

This is a first-class workflow.

Example (all source-development checkouts use generated values):

- main checkout
  - database: generated `patchbay_<checkout>_<offset>`
  - backend/frontend: generated isolated ports
- worktree checkout
  - database: generated `patchbay_my_feature_702`
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

Commands always target the current checkout:

```bash
pnpm dev
make stop
make check
make test
make migrate-up
make migrate-down
```

These generic targets require a valid env file in the current directory.

## How Database Creation Works

Database creation is automatic.

The following commands ensure the target database exists before they continue:

- `pnpm dev` / `make dev`
- `make test`
- `make migrate-up`
- `make migrate-down`
- `make check`

That logic lives in `scripts/ensure-postgres.sh`.

## Testing

Run the explicit full local verification pipeline:

```bash
make check
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

## Complete Desktop Runtime

The complete Desktop launcher owns the backend, browser login origin, daemon
runtime, database, ports, and Electron process for the current checkout. Use
one command for this path:

```bash
pnpm dev
```

It creates and validates `.env.worktree`, stages a source-matched CLI/backend/
migration set from the shared artifact cache (or performs one incremental Rust
build on a miss), starts the isolated database and services, runs capability
diagnostics, and then opens Electron. `pnpm dev:doctor` repeats diagnostics
without starting another stack.

Do not manually write CLI profiles, copy tokens into config files, or use an
ambient PATH-installed CLI for Desktop development. Browser login in Electron
exchanges the session for a Desktop-owned CLI token. A standalone terminal CLI
does require its own `patchbay setup`/`patchbay login`; keep that profile
separate from Desktop's profile.

If a generated `.env.worktree` is from an older workflow, the launcher fails
closed. Regenerate it once with `FORCE=1 make worktree-env`; never copy `.env`
into a source-development checkout.

### Desktop App Local Testing

Run the complete Electron development environment with one command:

```bash
pnpm dev
```

For Desktop UI development against the hosted Google login and API, use the
explicit hosted profile:

```bash
pnpm dev:hosted
```

The hosted profile keeps the Electron/Vite renderer local for hot reload, but
opens OAuth at `https://accounts.aspectlylabs.com` and sends API/WebSocket
traffic to `https://api.aspectlylabs.com`. It does not start a local database,
Rust server, or Next.js login origin. This profile is intentionally opt-in
because it can read and change shared hosted data. The launcher rejects
conflicting inherited `VITE_*` values before starting.

The command does not open Electron until it has:

1. Created or loaded the checkout-specific env, database, ports, and Electron
   `userData` identity
2. Installed dependencies using the global content-addressable pnpm store and
   this worktree's own `node_modules` link tree
3. Staged checksum-valid dev CLI, backend, and migration binaries whose Rust
   source, Cargo manifests/lockfile, toolchain, target, architecture, profile,
   and build metadata match
4. Applied migrations and reached the local backend's DB-backed `/healthz`
   readiness endpoint
5. Started the checkout's Next.js Web origin and pointed Desktop browser,
   share, and Google login links at that reachable service. Without the full
   Clerk configuration the launcher lists the missing values instead of
   silently emitting a dead login link
6. Exercised local agent discovery through `patchbay daemon probe-runtimes`
   (the renderer never guesses the host PATH)
7. Verified Telegram and Weixin credential-encryption keys without logging
   their values

There is deliberately no UI-only or released/PATH-CLI development fallback. A
missing capability fails before the window opens and prints an executable fix.
Use `pnpm dev:doctor` to repeat the local diagnostics while the stack is running,
or `pnpm dev:doctor --hosted` from a separate terminal to probe the hosted API
and accounts broker explicitly.

| Situation                                                                      | Command                                   | Expected work                                                                          |
| ------------------------------------------------------------------------------ | ----------------------------------------- | -------------------------------------------------------------------------------------- |
| Normal local product development                                               | `pnpm dev` or `make dev`                  | Complete Electron + dev CLI + backend + Web origin + isolated DB, with Vite hot reload |
| Desktop development against hosted OAuth/API                                  | `pnpm dev:hosted`                         | Local Electron/Vite hot reload with the production accounts/API tuple; no local API    |
| Re-run capability diagnostics                                                  | `pnpm dev:doctor [--hosted]`              | CLI/version/source, selected API/accounts endpoints, agent detection, Telegram/Weixin configuration |
| Compile-check frontend/Electron output                                         | `pnpm --filter @patchbay/desktop build`   | Electron/Vite production bundles; no Rust                                              |
| Validate an installer, signing/notarization, updater, embedded CLI, or release | `pnpm --filter @patchbay/desktop package` | Release Rust CLI and installer packaging; may take tens of minutes                     |

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

Complete Desktop development logs in through the browser Clerk development
flow. The launcher bootstraps the development Clerk configuration from the
approved secret store (or complete process-only Clerk variables) and fails
before Electron if that configuration is unavailable. Never put Clerk secrets
or CLI tokens in `.env` files, the repository, or chat.

Telegram still requires a real BotFather token entered through the integration
flow, and personal Weixin still requires a real iLink QR authorization. Without
those external credentials the UI must stop at the explicit provider step; a
configured encryption key is capability readiness, not proof of a successful
provider connection or test message.

An installation row with `status: active` means that provider authorization was
accepted; it is not a message-delivery guarantee. The Desktop status remains
“Authorized · test message required” until a real inbound provider message gets
a successful outbound response. The Telegram and Weixin adapters then record a
server-owned verification marker (`round_trip_status: passed`) atomically; a
failed send or a missing marker keeps the UI pending. A standalone terminal CLI
login is only needed for CLI commands that call a protected server directly;
Electron's complete dev flow signs in through the browser and owns its daemon
session separately. Do not copy a CLI token into the repository or an `.env`
file.

#### Running multiple checkouts side-by-side

`pnpm dev` auto-isolates every source-development checkout so several
checkouts can run their own desktop dev instance at once — no extra setup.
The generated `.env.worktree` records the allocated offset (the same offset
used for backend/frontend ports):

- `DESKTOP_RENDERER_PORT` = `5174 + offset` — its own Vite dev server. The
  one offset that would land on `6000` gets `6174` instead: Chromium treats
  `6000` as a restricted port and fails the load with `ERR_UNSAFE_PORT`
- `DESKTOP_APP_SUFFIX` = `<folder>-<offset>` — its own single-instance lock /
  `userData`, and an app named `Patchbay Canary <folder>-<offset>` so it is
  distinguishable in Cmd+Tab. The offset keeps it unique across worktrees that
  share a folder name at different paths.

The complete launcher exports each checkout's backend and WebSocket endpoints
to Electron, so no hand-written `apps/desktop/.env.development.local` is
required.

### Isolation Guarantee

Nothing in this flow touches the system-installed `patchbay` or the default
`~/.patchbay/config.json`:

| Resource        | System / Production            | Local Dev (per-checkout)                             |
| --------------- | ------------------------------ | ---------------------------------------------------- |
| Config          | `~/.patchbay/config.json`      | `~/.patchbay/profiles/desktop-<host>/config.json` |
| Daemon PID      | `~/.patchbay/daemon.pid`       | `~/.patchbay/profiles/desktop-<host>/daemon.pid`  |
| Health port     | `19514`                        | derived from the Desktop profile (never `19514`)  |
| Workspaces dir  | `~/patchbay_workspaces/`       | checkout-scoped task config root                  |
| Database        | remote / production            | local PostgreSQL: `patchbay_<slug>_<offset>`      |
| Desktop profile | `desktop-api.aspectlylabs.com` | `desktop-localhost-<port>`                        |

Multiple source-development checkouts can run simultaneously without sharing
databases, ports, Electron identities, or target directories.

## Troubleshooting

### Missing or Stale Development Env

If you see:

```text
Missing env file: .env.worktree
```

or a message saying that `.env.worktree` is not a current isolated checkout
environment, create/regenerate the file:

```bash
make worktree-env
```

For an existing stale file, use `FORCE=1 make worktree-env`. Only set
`PATCHBAY_DEV_ENV_FILE` when you intentionally want an explicit override
outside the generated complete-dev environment.

### Check Which Database a Checkout Uses

Inspect the env file:

```bash
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

### Checkout Is Accidentally Using the Main Database

The complete launcher validates the generated `.env.worktree` identity,
database name, and ports before starting. If validation fails, regenerate it
with `FORCE=1 make worktree-env` rather than copying `.env`.

### App Stops but PostgreSQL Keeps Running

That is expected.

`make stop` only stops the current checkout's tracked Electron/backend/Web
process tree.

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
make stop        # stop the tracked development process tree first
make db-reset
pnpm dev
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
- after that run `pnpm dev` again

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
pnpm dev
```

### Validate Before Pushing

```bash
make check
```
