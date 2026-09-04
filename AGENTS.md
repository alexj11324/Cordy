# Repository Guidelines

This file provides guidance to AI agents when working with code in this repository.

> **Single source of truth:** This file is the entry point for repository agent
> rules. Read **CLAUDE.md** at the project root for the expanded architecture,
> coding, testing, and release requirements referenced by this file.
> Use `Makefile`, `package.json`, and `pnpm-workspace.yaml` as the
> source of truth for the full command list.

## Quick Reference

### Architecture

Go backend + monorepo frontend (pnpm workspaces + Turborepo) with shared packages.

- `server/` - Go backend (Chi router, sqlc, gorilla/websocket)
- `apps/web/` - Next.js frontend (App Router)
- `apps/desktop/` - Electron desktop app
- `apps/mobile/` - Expo / React Native iOS app (read `apps/mobile/CLAUDE.md` first)
- `apps/docs/` - Fumadocs documentation site
- `packages/core/` - Headless business logic (Zustand stores, React Query hooks, API client)
- `packages/ui/` - Atomic UI components (shadcn/Base UI, zero business logic)
- `packages/views/` - Shared business pages/components
- `packages/tsconfig/` - Shared TypeScript config
- `packages/eslint-config/` - Shared ESLint config

### State Management (critical)

- **React Query** owns all server state (issues, members, agents, inbox, workspace list)
- **Zustand** owns client/view state (view filters, drafts, modals, desktop tab state); current workspace identity is route-driven and only mirrored for platform plumbing
- All Zustand stores live in `packages/core/` - never in `packages/views/` or app directories
- WS events update React Query for server data; store writes are only for clearing client-owned pointers with a single responder/self-event guard

### Package Boundaries (hard rules)

- `packages/core/` - zero react-dom, zero localStorage, zero process.env
- `packages/ui/` - zero `@patchbay/core` imports
- `packages/views/` - zero `next/*`, zero `react-router-dom`, use `NavigationAdapter` for routing
- `apps/web/platform/` - only place for Next.js APIs

### Database Migrations (hard rules)

- Never add database foreign keys or cascading actions. Enforce relationships and perform dependent cleanup explicitly in the application layer, using transactions when the operation must be atomic.
- Every index created by a migration, including unique indexes and indexes on new tables, must use `CREATE [UNIQUE] INDEX CONCURRENTLY`. Keep each concurrent index build in its own single-statement migration file.

### Commands

```bash
make dev              # Auto-setup + start everything
pnpm typecheck        # TypeScript check
pnpm test             # TS unit tests (Vitest)
make test             # Go tests
make check            # Full verification pipeline
```

See CLAUDE.md for the expanded rules and common commands incorporated by this
entry point.

### Agent Toolchains and Temporary Storage

- Keep the system `/tmp` tmpfs at 1 GiB. Do not resize or remount it for builds.
- Large or executable temporary build files must use a root-backed per-user
  directory such as `${XDG_CACHE_HOME:-$HOME/.cache}/codex-tmp-10g`; export that
  path as `TMPDIR` for the command. Provision it with roughly 10 GiB of free
  root-disk capacity, but do not treat the name as a quota guarantee.
- `/tmp` may be mounted `noexec`; never assume a compiler or test runner can
  execute binaries emitted there.
- Use the repository-required tool versions. Check existing per-user toolchain
  caches before downloading or upgrading tools; this branch currently targets
  Node 22 and the latest Go 1.26 patch selected by CI.
- Put Go download/build caches and the pnpm store on root-backed per-user cache
  paths. These immutable/content-addressed caches may be shared, while each
  worktree must retain isolated `node_modules`, build outputs, databases, ports,
  processes, and runtime state.
- Do not hard-code a developer's absolute home directory in scripts or tracked
  configuration. Resolve cache paths from `XDG_CACHE_HOME` or `HOME` and allow
  explicit environment overrides.
