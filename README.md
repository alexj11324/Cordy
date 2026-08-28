<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/patchbay/lockup-on-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/brand/patchbay/lockup-on-light.svg">
  <img alt="Patchbay" src="docs/assets/brand/patchbay/lockup-on-light.svg" width="320">
</picture>

**Route coding-agent work from intent to review without losing the thread.**

[![CI](https://github.com/alexj11324/patchbay/actions/workflows/ci.yml/badge.svg)](https://github.com/alexj11324/patchbay/actions/workflows/ci.yml)

**English | [简体中文](README.zh.md)**

</div>

Patchbay is an open-source control plane for coding-agent work. It keeps the
request, execution, decisions, result, and review state together while agents
run on infrastructure you control.

The name comes from a physical patch bay: a visible routing surface that
connects inputs and outputs without hiding the path between them.

## What Patchbay does

- **Keeps work connected.** Requirements, agent runs, progress, blockers, and
  review live on the same issue.
- **Runs agents where the code lives.** A local daemon launches authenticated
  coding-agent CLIs on your machine or a runtime you operate.
- **Makes execution inspectable.** Follow events, logs, retries, timeouts, and
  usage instead of reconstructing a terminal session later.
- **Preserves human control.** Completed work returns for review before it is
  accepted or shipped.
- **Fits different work surfaces.** The repository contains web, desktop, and
  iOS clients plus a CLI and API.
- **Can be self-hosted.** Run the application and PostgreSQL on your own
  infrastructure.

Patchbay does not bundle a model or coding agent. It coordinates compatible
agent CLIs that you install and authenticate separately.

## Architecture

```text
 Web · Desktop · iOS
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
                            Installed coding-agent CLI
```

| Layer | Current implementation |
| --- | --- |
| Web | Next.js App Router |
| Desktop | Electron with shared web UI packages |
| Mobile | Expo / React Native for iOS |
| Backend | Rust, Axum, SQLx, and WebSocket |
| Database | PostgreSQL 17 with pgvector |
| Local runtime | Rust CLI and daemon launching installed agent CLIs |

The Rust server, CLI, migration runner, and backfill binaries are the default
production entrypoints. Legacy Go source remains temporarily as migration
evidence and for compatibility checks; its final removal is tracked in the
[Go-to-Rust migration audit](tasks/go-to-rust-migration-audit.md).

## Run from source

### Prerequisites

- Node.js 22+
- pnpm 10.28.2
- a stable Rust toolchain
- Docker with Docker Compose for PostgreSQL

Go is only needed for the temporary legacy compatibility suite while the final
migration gate remains open.

```bash
git clone https://github.com/alexj11324/patchbay.git
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
- [Migration audit](tasks/go-to-rust-migration-audit.md)

Some internal package, executable, environment-variable, and storage names
still use the previous product identifier. They are retained where changing a
public or persisted contract would be unsafe and will be handled as a separate
rename boundary.

## License

Patchbay is distributed under the terms in [LICENSE](LICENSE). Attribution
notices are in [NOTICE](NOTICE).
