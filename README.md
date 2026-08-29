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

Patchbay is an open-source control panel for orchestrating multi-agent systems on long-horizon tasks. It automatically decomposes your ambitious goals into actionable tasks, builds their dependency graph, and schedules execution until the work is complete.

## What Patchbay Can Do

- **Auto Decompose Tasks** Your goals will be decomposed into actionable tasks depending on the dependency by an agent with well organized prompt.
- **Kanban** Each task will be available on the Kanban, so you and your team could track the progress of the project on one plane.
- **End to end tasks orchestration** A live agent orchestrator monitors the progress of each tasks and schedules the execution.
- **Bring your own subscriptions** All tasks running on your local agents via Agent client protocol(ACP)
- **Makes tasks interactive.** All actions logs on the tasks page as a thread. You can assign any task to any Agent and team member at any time.
- **Human in the loop** Steer agent when drifting.
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

| Components | Current implementation |
| --- | --- |
| Web | Next.js App Router |
| Desktop | Electron with shared web UI packages |
| Mobile | Expo / React Native|
| Backend | Rust, Axum, SQLx, and WebSocket |
| Database | PostgreSQL 17 with pgvector |
| Local runtime | Local daemon launching installed agent via ACP |

## Run from source

### Prerequisites

- Node.js 22+
- pnpm 10.28.2
- a stable Rust toolchain
- Docker with Docker Compose for PostgreSQL

```bash
git clone https://github.com/patchbay-ai/patchbay.git patchbay
cd patchbay
make dev
```

`make dev` creates the local environment when needed, installs dependencies,
starts PostgreSQL, applies migrations, and launches the Rust backend and web
client.

For an explicit build:

```bash
make build
pnpm build
```

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
