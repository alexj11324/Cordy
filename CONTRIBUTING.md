# Contributing Guide

This guide documents the local development workflow for contributors working on the Patchbay codebase.

It covers:

- first-time setup
- environments: starting, inspecting, stopping and deleting them
- day-to-day development in the main checkout
- isolated worktree development
- the shared PostgreSQL model
- testing and verification
- full-stack isolated testing (backend + frontend + daemon from source)
- troubleshooting and destructive reset options

## Day One

Two commands, from a fresh clone:

```bash
make up C=desktop   # backend + the Electron app, already signed in
make seed-dev       # optional: sample issues, in the dev-fixtures workspace
```

**Changes are verified in the desktop app, not the browser.** `make up C=desktop` starts Electron
against this checkout's backend and signs it in, so that is where you look at what you built. Add
`make up C=api,web` (and `make dev-login` to get a signed-in browser) when the change is web-only
platform wiring.

`make status` shows what is running and proves it belongs to this checkout, `make down` stops it
keeping the database, and `make destroy` deletes the database, profile and slot. Everything below
expands on those; [Environments](#environments) is the section to read first.

## Contribution Terms

By submitting a contribution to Orvilo — a pull request, a patch, or any
other work — you agree to condition 2 of the [Orvilo License](LICENSE):

- your contribution is submitted under the Orvilo License as a whole (the
  additional conditions in Part I together with the incorporated Apache
  License 2.0 text in Part II), not under the Apache License 2.0 alone;
- your contributed code may be used for commercial purposes, including the
  producer's cloud business operations;
- the producer can adjust the Orvilo License to be more strict or relaxed
  as deemed necessary.

See the [LICENSE](LICENSE) file for the full terms.

## Development Model

Local development uses one shared PostgreSQL container and one database per checkout.

- the main checkout usually uses `.env` and `POSTGRES_DB=patchbay`
- each Git worktree uses its own `.env.worktree`
- every checkout connects to the same PostgreSQL host: `localhost:5432`
- isolation happens at the database level, not by starting a separate Docker Compose project
- backend and frontend ports are still unique per worktree

This keeps Docker simple while still isolating schema and data.

## Prerequisites

- Node.js `22`
- `pnpm` `10.28.2`
- Go `1.26.6`
- Docker

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

## Environments

An environment is the database, ports, CLI profile and processes that belong to
one checkout. It is a named object: it can be listed, inspected and deleted.

```bash
make up                      # start this checkout's environment (api + web)
make up C=api,web,daemon     # choose the components
make status                  # what is running, and whether it is yours
make list                    # every environment on this machine
make down                    # stop the processes, keep the data
make destroy                 # stop, then drop the database and free the slot
make gc                      # collect expired environments or ones whose checkout is gone
make dev-login               # sign in without the login page
```

Components are `api` (Go backend), `web` (Next.js), `daemon` (agent daemon) and
`desktop` (Electron). Selecting any of them implies `api`. `make up` is
idempotent: re-running it against a live environment reuses the database, the
profile and any component already healthy.

Three properties are worth knowing because the old flow lacked them:

- **API, Web and Desktop renderer ports, database names and profiles are allocated, not recomputed.** The
  allocator starts from this directory's path hash, so a checkout keeps the
  numbers it has always had, and moves only when the registry or a live
  listener says the slot is taken. The registry lives in `~/.patchbay/dev/`;
  deleting it and re-running `make up` is a supported recovery.
- **Nothing reports success for something it has not reached.** The database is
  created and verified through `DATABASE_URL` — the same string the backend
  uses — and `GET /health` reports `pid`, `commit` and `started_at` so `make up`
  can prove the process answering is the one it just started rather than a
  leftover on the same port.
- **`down` and `destroy` differ deliberately.** `down` stops processes and
  keeps the database, profile and slot, so the next `make up` is seconds.
  `destroy` consumes the database, profile, daemon task workspaces, Desktop
  userData and slot. If any deletion fails, it keeps the manifest and exits
  non-zero so cleanup can be retried instead of losing the deletion recipe.
- **Temporary environments have a best-effort fallback.** `make up
  ARGS=--ephemeral` records a 24-hour TTL. The next `make up` automatically
  collects expired and directory-less environments; `make gc` runs the same
  collection explicitly.

Run any command inside an environment's variables without repeating them:

```bash
make env-exec ARGS="-- pnpm exec playwright test"
```

### Signing in without the login page

`make up` writes `ORVILO_DEV_LOGIN=1` into the env file, which makes the
backend serve `/auth/dev-login`. `make dev-login` uses it and prints:

- a URL that installs the session cookie and lands on this environment's issues
  page — opening it *is* the login, so there is no code to fetch and no form to
  fill;
- a bearer token for `curl`, and the workspace it signed you into. The dev
  workspace is created on the first login if it does not exist yet.

```bash
make dev-login                                        # dev@localhost
make dev-login ARGS="--email you@example.com --open"  # another user, open the browser
make dev-login ARGS="--path /dev/inbox"               # land somewhere else
make dev-login ARGS=--json                            # url + token for a script
```

The endpoint exists only when `ORVILO_DEV_LOGIN=1` and `APP_ENV` is
non-production; a production build does not register the route at all. Add
`?onboarding=keep` to the URL when you want to test the onboarding flow itself.
Backends started before this variable was in the env file need one
`make down && make up` to pick it up.

`make dev` (below) still runs backend and frontend in the foreground of your
terminal, which is the right thing when you want Ctrl-C to stop everything.

## First-Time Setup

### Quick Start (recommended)

From any checkout (main or worktree):

```bash
make up
```

This single command:

- auto-detects whether you're in a main checkout or a worktree
- creates the appropriate env file (`.env` or `.env.worktree`) if it doesn't exist
- checks that prerequisites (Node.js, pnpm, Go, Docker) are installed
- installs JavaScript dependencies
- allocates this checkout's ports and database name under a lock, and records them in
  `~/.patchbay/dev/` so the environment can be listed, inspected and deleted later
- creates the application database if it does not exist and runs all migrations
- starts the selected components (`api,web` by default) in the background

Then `make dev-login` to get into the app, and `make seed-dev` if you want sample content.

#### `make dev`: backend and Electron in the foreground

```bash
make dev
```

`make dev` does the same setup but runs the backend and opens Electron in the foreground of your
terminal, which is what you want when Ctrl-C should stop everything. It does **not** start the web
app — use `make up C=api,web` for that — and it does not register an environment, so `make status`,
`make list` and `make destroy` do not track it.

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
make up
```

`make up` detects the worktree, generates `.env.worktree`, and allocates ports and a database
that do not collide with the main checkout.

After that, day-to-day commands are:

```bash
make up               # start (idempotent: reuses anything already healthy)
make dev-login        # sign in to this worktree's environment
make status           # what is running here, with proof it is this checkout's
make down             # stop, keeping the database
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
make dev-login
make seed-dev
make test
make migrate-up
make migrate-down
```

These generic targets require a valid env file in the current directory.

`make seed-dev` installs deterministic sample content — issues with a dependency graph — into a
separate `dev-fixtures` workspace, so it never mixes with whatever you are building in your own
workspace. It needs the developer user to exist, which `make dev-login` creates: run the two in
that order. The seed is idempotent and preserves rows you edited after the first run.

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
3. Go tests
4. Playwright E2E tests

Notes:

- Go tests create their own fixture data
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

Running the complete stack — backend, frontend and daemon — from source, with
its own database and CLI profile, is one command:

```bash
make up C=api,web,daemon
```

It creates the environment if needed, sets the fixed local verification code
before the first launch, logs in as `dev@localhost`, mints a personal access
token, creates a workspace, writes the CLI profile, builds `server/bin/patchbay`
and starts the daemon from that binary. It then prints the URL, the login, the
commit, and the stop command.

Two constraints are enforced rather than documented:

- **The daemon runs from a built binary, never `go run`.** The daemon records
  its own executable path at startup and re-execs it as the
  execution-environment helper for every task; `go run` deletes that binary when
  the launcher exits, so the daemon would register, heartbeat, and then fail
  every task with `fork/exec …/go-build…/exe/patchbay: no such file or directory`.
- **`daemon start` is refused under a daemon-managed task.** A checkout below a
  `.patchbay/daemon_task_context.json` marker cannot start a second daemon
  competing for its own work, so `make up C=daemon` stops with that explanation
  before spending a login on it. Use `C=api,web` there.

### Desktop

```bash
make up C=desktop
```

This writes a marked `apps/desktop/.env.development.local` pointing at this
environment's backend, starts Electron with the renderer port and app name from
the environment registry, and waits until that renderer is actually serving.
Several checkouts can therefore run Desktop side by side without maintaining a
second path-derived identity. `make destroy` removes the marked env file and
this environment's Electron userData. Direct `pnpm dev:desktop` still uses its
path-derived fallback when it is run outside `make up`.

Desktop starts signed in: `make up C=desktop` mints a token from `/auth/dev-login` and writes it
into the gitignored `apps/desktop/.env.development.local` as `VITE_DEV_LOGIN_TOKEN`, which the
renderer seeds into storage at boot. It also creates the dev workspace if this environment has
none, so Electron opens on a usable screen rather than the create-workspace flow.

This is deliberately not the same mechanism as the browser: Desktop authenticates with a stored
bearer token, so the HttpOnly cookie `make dev-login` installs does nothing for it. An existing
session always wins — the seed only fills an empty storage, so it never logs you out of an account
you are testing with.

If the token could not be minted (a backend started before `ORVILO_DEV_LOGIN=1` was in the env
file), `make up C=desktop` says so and Electron shows the login page; `make down && make up
C=desktop` fixes it. You can always fall back to `dev@localhost` with code `888888` on that page.

To exercise the onboarding flow itself — which starts from a user who has not completed it — run
`ORVILO_DEV_KEEP_ONBOARDING=1 make up C=desktop`. The browser equivalent is `?onboarding=keep` on
the URL `make dev-login` prints.

### Isolation Guarantee

Nothing in this flow touches the system-installed `patchbay`, the default
`~/.patchbay/config.json`, or the public production app. Staging is a third
channel with its own hosted backend — see
[docs/operations/environments.md](docs/operations/environments.md).

| Resource | Public / Production | Testing / Staging | Local Dev (per environment) |
|---|---|---|---|
| Config | `~/.patchbay/config.json` | `~/.patchbay/profiles/staging/config.json` | `~/.patchbay/profiles/dev-<slug>-<offset>/config.json` |
| Daemon PID | `~/.patchbay/daemon.pid` | `~/.patchbay/profiles/staging/daemon.pid` | `~/.patchbay/profiles/dev-<slug>-<offset>/daemon.pid` |
| Workspaces dir | `~/patchbay_workspaces/` | `~/patchbay_workspaces_staging/` | `~/patchbay_workspaces_dev-<slug>-<offset>/` |
| Database | production | staging (`patchbay-staging` Compose project) | local: `patchbay_<slug>_<offset>` |
| Desktop app | `Orvilo` | `Orvilo Staging` | `Orvilo Canary` |
| Desktop callback | `patchbay://` | `patchbay-staging-<hash>://` | `patchbay-canary-<hash>://` |
| Desktop profile | `desktop-api.aspectlylabs.com` | `desktop-api.staging.aspectlylabs.com` | `desktop-localhost-<port>` |
| API | `https://api.aspectlylabs.com` | `https://api.staging.aspectlylabs.com` | local backend port |
| Registry | — | `/var/lib/patchbay-staging/` on the origin | `~/.patchbay/dev/envs/<name>/` |

Multiple environments run simultaneously without conflict; `make list` shows
all of them.

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
`y/N` confirmation. It only operates on the local Docker PostgreSQL service,
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
make up
make dev-login
```

### Feature Worktree

```bash
git worktree add ../patchbay-feature -b feat/my-change main
cd ../patchbay-feature
make up
make dev-login
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
