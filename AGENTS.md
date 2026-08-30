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
make web-dev          # Vite-hosted shared Desktop renderer + local preview API
make web-next-dev     # Next.js web frontend
make api-dev          # Rust API only (set VITE_API_URL when using it with Vite)
pnpm typecheck        # TypeScript check
pnpm test             # TS unit tests (Vitest)
make test             # Developer helper for Rust tests; GitHub Actions is authoritative
make check            # Product-wide local helper; not an agent/default migration gate
```

### Local Desktop UI Preview

To open the shared Desktop renderer without starting the full backend, run
this from the repository root:

```bash
DESKTOP_RENDERER_PORT=5175 pnpm --filter @patchbay/desktop dev:web
```

Then open [http://127.0.0.1:5175/ui-preview](http://127.0.0.1:5175/ui-preview)
in a browser. The Vite host installs a same-origin preview API for read-only
fixture data, so the page is an explicit local demo and does not require
PostgreSQL, a daemon, or a live automation backend. Its banner identifies the
data as sample data and says when the backend is not connected. The sample issue cards are real
shared issue-surface links: click one or focus it and press Enter to open the
issue detail, where the linked execution log and task handoff state are shown.
The sample runs stay on that shared issue path; the preview does not add a
standalone transcript or pretend that read-only run details are persisted.
From the preview sidebar, choose **Autopilot** to inspect the sample
automation list and open a row for its run/detail state. The preview keeps
these workspace tabs in an in-memory router, so `/preview/autopilots` is an
internal tab path rather than a URL to paste into the browser. Product data
writes are intentionally unsupported and fall through to Vite; view
preferences may only update in memory for the current tab. Do not treat this
preview as persisted or live automation data. Set `VITE_API_URL` only when
deliberately testing the renderer against a real backend.

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

### Verification scope

GitHub Actions is the authoritative CI, compilation, and test environment for
Rust and repository-wide checks. After pushing a PR branch, use its GitHub
checks for Rust formatting, check, Clippy, tests, builds, deployment contracts,
production images, installers, and platform coverage. Diagnose failures from
the Actions logs, push a fix, and wait for the replacement run; local results
must not be used as a substitute for a required GitHub check.

For frontend and Desktop changes, local verification is part of the definition
of done. Run the narrowest relevant typecheck, unit test, build, and dev-server
or application smoke check for the affected path when that behavior is in
scope. Local `pnpm` commands, Vitest, Playwright, Vite dev servers, and
frontend/Desktop builds are allowed for this purpose and complement, rather
than replace, GitHub checks. Do not run unrelated full-repository pipelines
just for confidence.

Agents must not run local `cargo`, Go commands, Docker builds, `make test`,
`make check`, or any Rust compilation/test pipeline. Rust migration work must
never run Go tooling or Go tests. Default tests and production code must also
never discover or execute ambient, user-installed agent CLIs; tests must supply
a fixture executable or an explicitly missing path.

### Default feature delivery loop

For an implementation request, the default definition of done is a complete
GitHub delivery loop, not merely a local commit. Unless the user explicitly
asks for analysis-only work, a local-only change, or no merge, agents must:

1. Work in a dedicated worktree and focused branch, preserving unrelated
   changes in every other worktree.
2. Perform the required local checks for the affected path plus lightweight
   hygiene checks, commit the focused concern, push the branch, and create or
   continue the corresponding PR.
3. Wait for every required GitHub Actions check. If a check fails, inspect its
   logs, fix the earliest responsible cause on the same branch, push the fix,
   and wait for the replacement checks. Never treat a local build or test as a
   substitute for a required GitHub check.
4. Read and address every valid review comment, then re-run the affected
   checks. Do not leave unresolved review threads hidden behind a passing CI
   run.
5. Perform the real runtime or deployment acceptance requested by the task
   before merging. A green CI run alone is not runtime evidence when the task
   includes a live service, callback, browser, desktop, or deployment path.
6. Merge the PR using the repository's required merge method only after all
   required checks, review requirements, and requested runtime acceptance are
   satisfied. Report the merge commit or resulting `main` state, not just the
   branch commit.

This loop is iterative: “PR 创建后” means continue through CI failures,
review fixes, and replacement runs until the PR is mergeable. If DNS, external
service availability, credentials, branch protection, required human approval,
or another dependency prevents an honest acceptance, stop before merging and
report the exact blocker and the next required action. Never claim a PR was
merged, CI/CD completed, or runtime verified when the evidence is absent.

### Commits, PRs, and Releases

- Use focused conventional commits (`feat`, `fix`, `refactor`, `docs`, `test`,
  `chore`) and preserve the repository's required merge method.
- Do not stop after a local edit or partial check. Push the correct PR branch,
  wait for all required GitHub Actions jobs, fix failures, and merge only after
  valid review comments are handled and no real blocker remains.
- A production release is created from a version tag on `main`; the release
  workflow publishes binaries, images, installers, charts, and the stable
  Homebrew formula after its required jobs succeed.
