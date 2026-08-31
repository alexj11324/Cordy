<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/patchbay/lockup-on-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/brand/patchbay/lockup-on-light.svg">
  <img alt="Patchbay" src="docs/assets/brand/patchbay/lockup-on-light.svg" width="320">
</picture>

**End to End Multi-Agent Harness for long-Horizon tasks**

[![CI](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml/badge.svg)](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml)

**English | [简体中文](README.zh.md)**

</div>

Patchbay is an open-source Harness for orchestrating multi-agents on long-horizon tasks. It automatically decomposes your ambitious goals into actionable tasks, builds their dependency graph, and schedules execution until the work is complete.

## What Patchbay Can Do

- **Auto Decompose Tasks** Your goals will be decomposed into actionable tasks depending on the dependency by an agent with well organized prompt.
- **Kanban** Each task will be visible on tasks Kanban, so you and your team could track the progress of the project on one plane.
- **End to end tasks orchestration** A live agent orchestrator monitors the progress of each tasks and schedules the execution.
- **Bring your own subscriptions** All tasks running on your local agents via Agent client protocol(ACP)
- **Makes tasks interactive.** All actions logs on the tasks page as a thread. You can assign any task to any Agent and team member at any time.
- **Human in the loop** Your threads are always under control, steer agent when drifting.
- **Work on your preferred platform** Web, desktop, Mobile, CLI and API.
- **Self-host** running the program on your own infrastructure.

## Architecture

```text
 Web · Desktop · Mobile · CLI
          │
          ▼
 Next.js / shared UI ───────► Rust API + WebSocket server
                                      │
                                      ▼
                              PostgreSQL + pgvector
                                      ▲
                                      │ task events
                               Local agent daemon
                                      │
                                      ▼
                               Agents via ACP
```

| Components    | Current implementation                         |
| ------------- | ---------------------------------------------- |
| Web           | Next.js App Router                             |
| Desktop       | Electron with shared web UI packages           |
| Mobile        | Expo / React Native                            |
| Backend       | Rust, Axum, SQLx, and WebSocket                |
| Database      | PostgreSQL 17 with pgvector                    |
| Local runtime | Local daemon launching installed agent via ACP |

## Run from source

### Prerequisites

- Node.js 22 (the repository pins `>=22 <23`)
- pnpm 10.28.2
- a stable Rust toolchain
- sccache (`brew install sccache` on macOS)
- Docker with Docker Compose, or a native PostgreSQL 15+ installation

```bash
git clone https://github.com/patchbay-ai/patchbay.git patchbay
cd patchbay
pnpm dev
```

`pnpm dev` (or the POSIX convenience alias `make dev`) is the single complete
Desktop development entry on macOS, Linux, and Windows. It
creates an isolated worktree environment, installs dependencies through the
shared pnpm store, starts PostgreSQL, applies migrations, waits for the local
Rust backend and database, prepares a source-matched dev runtime (CLI, backend,
and migration runner), verifies local
agent detection plus Telegram/Weixin encryption configuration, and only then
opens Electron with renderer hot reload.

The dev runtime is cached per user by Rust source, Cargo manifests/lockfile,
toolchain, target, architecture, profile, and build metadata. Repeated starts
and new worktrees with the same Rust source reuse all three verified binaries
without compiling Rust. A cache miss builds them together once in the
incremental dev profile; install `sccache`
to share compiler outputs while each worktree keeps an independent
`server-rs/target`. Run `pnpm dev:doctor` to repeat the capability checks. For
the separate Next.js web client, use `pnpm dev:web:next`.
`PATCHBAY_POSTGRES_RUNTIME=auto` uses Docker only for its published
`localhost:5432` endpoint; set it to `native` or `docker` when that endpoint is
intentionally ambiguous.
`make stop` terminates the tracked Electron, renderer, backend, and launcher
process tree for only the current checkout; it does not stop shared PostgreSQL.

For an explicit build:

```bash
make build
pnpm build
```

`pnpm build` builds the frontend and Electron bundles without compiling Rust.
Formal Desktop packaging still uses `pnpm --filter @patchbay/desktop package`,
which builds and embeds a release Rust CLI (or consumes a checksum-verified
exact-commit CI artifact) before creating installers.
Run that full path only for installer, signing/notarization, updater,
embedded-CLI, or release acceptance; it may take tens of minutes and is not a
day-to-day edit-refresh command. See [Contributing](CONTRIBUTING.md#desktop-app-local-testing)
for the complete path-selection table.

## Documentation

- [Self-hosting](SELF_HOSTING.md)
- [CLI and agent daemon](CLI_AND_DAEMON.md)
- [Contributing](CONTRIBUTING.md)
- [Advanced self-hosting configuration](SELF_HOSTING_ADVANCED.md)

The CLI, package scope, Rust crates, deployment artifacts, environment
variables, storage keys, and application identifiers all use the Patchbay
name. Existing local configuration and authenticated browser sessions are
migrated at their compatibility boundaries during the upgrade.

## License

Patchbay is distributed under the terms in [LICENSE](LICENSE). Attribution
notices are in [NOTICE](NOTICE).
