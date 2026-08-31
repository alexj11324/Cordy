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

### Refactor completeness invariant (repository-wide)

Any refactor or rename of a product concept, resource, feature, domain object,
name, or workflow must begin with an inventory and update every affected
layer: user interface and accessibility text, frontend models and queries,
API routes/payloads/schemas/SDKs, backend types and services, database tables,
columns, indexes and migrations, events and permissions, configuration and
CLI, telemetry, documentation, fixtures, tests, and generated artifacts.
The previous name must not remain the internal canonical source after the
change. A rolling upgrade may retain a legacy spelling only inside an isolated
deprecation adapter that is observable, tested, has an owner and an explicit
deletion deadline and condition; it must not create an indefinite dual-write
or dual-name contract. Every PR must list the unique remaining legacy
locations, their reason, owner, deletion condition, and the verification that
full-tree search, schemas/generated output, upgrade/downgrade, and the real
affected path all pass.

For the current product rename, `Automation` / `自动化` is the only canonical
concept in shipping code and contracts. Do not add new APIs, database objects,
types, events, permissions, telemetry labels, or UI using the former product
spelling. This product has no production users, external durable URLs, or
rolling deployment requiring a compatibility bridge, so no deprecation adapter
is authorized for this rename; immutable historical migrations/changelogs are
the only allowed old product-spelling residuals. A future adapter requires a
separate approved deployment decision and must satisfy the invariant above.

### Database Migrations (hard rules)

- Never add database foreign keys or cascading actions. Enforce relationships and perform dependent cleanup explicitly in the application layer, using transactions when the operation must be atomic.
- Every index created by a migration, including unique indexes and indexes on new tables, must use `CREATE [UNIQUE] INDEX CONCURRENTLY`. Keep each concurrent index build in its own single-statement migration file.

### Commands

These are developer commands, not the default agent verification path:

```bash
make dev              # Complete Electron + dev CLI + backend + isolated DB
make web-next-dev     # Next.js web frontend
make api-dev          # Rust API only (set VITE_API_URL when using it with Vite)
pnpm dev:doctor       # Re-run complete Desktop capability diagnostics
pnpm typecheck        # TypeScript check
pnpm test             # TS unit tests (Vitest)
make test             # Developer helper for Rust tests; GitHub Actions is authoritative
make check            # Product-wide local helper; not an agent/default migration gate
```

### Desktop development and build paths (hard rules)

| Situation                                                                     | Command                                                           | Rust work                                                                                                         |
| ----------------------------------------------------------------------------- | ----------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Normal local product development                                              | `pnpm dev` or `make dev`                                          | Source-matched CLI/backend/migration cache hit; one incremental development build only on a Rust fingerprint miss |
| Ordinary frontend/Electron build verification                                 | `pnpm --filter @patchbay/desktop build` or `pnpm build`           | None                                                                                                              |
| Installer, signing, notarization, updater, bundled-CLI, or release acceptance | `pnpm --filter @patchbay/desktop package` or the release workflow | Full release CLI plus packaging; may take tens of minutes                                                         |

- The standard development entry is intentionally complete: it prepares the
  isolated worktree database and ports, starts the local backend, stages a
  source-matched dev CLI/backend/migration set, verifies runtime discovery and integration encryption
  configuration, then opens Electron with hot reload. There is no renderer-only
  or released/PATH-CLI fallback development mode.
- Dev runtime artifacts are shared through a content-addressed per-user cache keyed
  by Rust source, manifests/lockfile, toolchain, target, architecture, profile,
  and build metadata. Each worktree keeps its own `server-rs/target` and
  `node_modules` links. Cargo downloads, pnpm's global store, sccache objects,
  and exact-match dev runtime artifacts may be shared; databases, ports, processes,
  target directories, and node_modules trees may not.
- Under the repository rule prohibiting local Cargo for agents, agents verify
  the complete launcher with focused script tests and GitHub Actions; human
  developers run `pnpm dev` for real local runtime acceptance.
- Use the full package/release path only when the requested acceptance actually
  concerns a distributable artifact or one of its production boundaries. The
  long release build belongs in GitHub Actions or a deliberate release check,
  never in the default edit-refresh loop.
- Never add a release CLI build to `dev` or ordinary `build`. `bundle-cli.mjs`
  requires an explicit `dev` or `release` profile. Development may use only the
  dev artifact cache/incremental profile; the formal package wrapper must pass
  `release` explicitly or consume a checksum-verified exact-commit release CLI
  artifact from the same workflow.

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

### Authorized execution invariants

- When the user has explicitly authorized a target and scope, the agent must
  autonomously complete every executable repair, deployment, and real-runtime
  acceptance within that scope. Ordinary login, password-manager or iCloud
  Keychain use, OTP entry, available cloud control planes, CI/review, DNS, and
  deployment steps are execution work, not blockers.
- When an authorized step fails, diagnose the concrete cause, repair it on the
  approved path, and continue. Do not stop at source inspection, a green CI
  result, or a deployment plan when the requested runtime or hosted result is
  still executable and unverified.
- Pause and request user takeover only for a platform-enforced Passkey,
  biometric, or equivalent security confirmation that cannot be covered by the
  user's authorization. Never bypass a platform-enforced security control,
  weaken an access boundary, or disclose passwords, OTPs, keys, tokens, or
  other secrets.

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
