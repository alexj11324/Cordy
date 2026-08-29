# Repository Guidelines

This file is the single source of truth for AI agents working in this
repository. Do not duplicate or maintain agent rules in `CLAUDE.md`; that file
is only a compatibility pointer back here. Use `Makefile`, `package.json`, and
`pnpm-workspace.yaml` as the source of truth for the full command list.

## Quick Reference

### Architecture

Rust backend + monorepo frontend (pnpm workspaces + Turborepo) with shared packages.

- `server-rs/` - production Rust backend, migration runner, daemon, and CLI
- `migrations/` - production database migrations consumed by the Rust runner
- `apps/web/` - Next.js frontend (App Router)
- `apps/desktop/` - Electron desktop app
- `apps/mobile/` - Expo / React Native iOS app (read `apps/mobile/AGENTS.md` first)
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

These are developer commands, not the default agent verification path:

```bash
make dev              # Auto-setup + start everything
make web-dev          # Next.js + local fixture API; real product UI, no Rust
make api-dev          # Rust API only (pair with PATCHBAY_UI_FIXTURES=0 make web-dev)
pnpm typecheck        # TypeScript check
pnpm test             # TS unit tests (Vitest)
make test             # Developer helper for Rust tests; GitHub Actions is authoritative
make check            # Product-wide local helper; not an agent/default migration gate
```

### Coding Conventions

- TypeScript is strict. Prefer `type` over `interface`, avoid `any`, and use
  `import type` for type-only dependencies.
- Reuse the shared API client and schemas rather than calling `fetch` directly
  from product code. Treat response enums and unions as forward-compatible:
  preserve unknown values with an explicit fallback instead of silently
  dropping them.
- Add reusable visual primitives to `packages/ui`; keep business logic in
  `packages/core` and shared product screens in `packages/views`.
- All user-facing strings must use the repository i18n layer. Update the
  English, Chinese, Japanese, and Korean locale files together.
- Use kebab-case filenames, PascalCase components, camelCase functions, and
  `use-*.ts` for hooks.

### Worktrees and Local Services

- Worktrees share PostgreSQL infrastructure but use isolated databases and
  ports through `.env.worktree`; never point one worktree at another's database.
- Do not delete or overwrite another worktree's files, build outputs, or running
  services. Preserve unrelated user changes in dirty worktrees.
- Default tests and production code must never discover or execute ambient,
  user-installed agent CLIs. Tests must supply a fixture executable or an
  explicitly missing path.

### CI and local verification scope

GitHub Actions is the sole CI, release-compilation, and test environment for this
repository. After pushing a PR branch, use its GitHub checks for Rust formatting,
check, Clippy, tests, builds, deployment contracts, production images, installers,
and platform coverage. Diagnose failures from the Actions logs, push a fix, and
wait for the replacement run; do not substitute a local result for a required
GitHub check.

Local Web development is allowed for runtime UI acceptance. Agents may start
the `apps/web` development server with `make web-dev` (including Next.js
on-demand compilation) and inspect it in a browser. `make web-dev` serves a
local fixture API so the real onboarding and app routes render without Rust.
Do not paint substitute screens; open `/onboarding` and `/{slug}/issues`.
This exception does not authorize local test suites, production builds,
desktop/mobile packaging, or release signing.

Agents must not run local `cargo`, Vitest/Playwright, Go commands, Docker builds,
`make test`, `make check`, or any other compilation or test pipeline outside the
local Web-development exception above. Rust migration work must never run Go
tooling or Go tests.

### Commits, PRs, and Releases

- Use focused conventional commits (`feat`, `fix`, `refactor`, `docs`, `test`,
  `chore`) and preserve the repository's required merge method.
- Do not stop after a local edit or partial check. Push the correct PR branch,
  wait for all required GitHub Actions jobs, fix failures, and merge only after
  valid review comments are handled and no real blocker remains.
- A production release is created from a version tag on `main`; the release
  workflow publishes binaries, images, installers, charts, and the stable
  Homebrew formula after its required jobs succeed.
