# Rust → Go product parity audit

Updated: 2026-09-03. Owner: Codex takeover on PR [#729](https://github.com/alexj11324/Cordy/pull/729).

## Scope and evidence

The user confirmed Rust → Go to shorten the backend build/development loop.
The React Web/Desktop surfaces must retain their behavior; a language change is
not authorization to remove a feature. No comparative build-speed benchmark has
been recorded yet.

The source reference is Rust `9df7c06cf767d599c697656a1d43d0eab3e3dea2`
(`origin/archive/rust-mainline`, also the inspected `origin/main`). The Go
continuation is `hoplite/kalymna-ed979cf8`, on the #727 → #728 → #729 stack.
This document is a **partial, evidence-backed inventory**, not a completed
whole-product audit or an estimate of all remaining work. The historical
[W4–W9 plan](MIGRATION_PLAN_W4_W9.md) and the original Hoplite PR report are
checkpoints, not proof that their acceptance gates passed in the current tree.

Delivery checkpoint: GitHub's event history attributes the closure of #729 at
2026-09-03T14:30:02Z to `usehoplite[bot]`. Codex restored the original PR under
the user's existing takeover authorization; Hoplite access remains revoked.
CI [33768750201](https://github.com/alexj11324/Cordy/actions/runs/33768750201) on
`dd9619d8985372f351d52aece294bb598dee8fad` passed backend build, migrations,
PostgreSQL-backed race tests, frontend and platform checks. Its only failure
was sqlc output drift. `a25efa85a` applies the exact generated diff; replacement
CI [33769723534](https://github.com/alexj11324/Cordy/actions/runs/33769723534) on
`d7d800f04` passed every applicable job, including sqlc and backend race tests.
Only the unchanged image-budget job was skipped. No production deployment or
final merge has occurred.

Backend compilation, SQL generation and tests run in GitHub Actions. No local
Go/Rust tooling or Docker build is part of this audit. Local frontend checks
and browser fixtures complement CI; neither proves real provider connectivity,
native UI completion, or production deployment.

## Inspected slices

| Slice                         | Current evidence                                                                                                                                                                                                                                                                                                                                         | Remaining acceptance                                                                                                   |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| Onboarding project attachment | `2c3336e46` restores repository URLs, native multi-folder consent bridge, daemon-bound local resources, target-workspace headers and four locales. Focused tests/typechecks/builds and a real-browser fixture passed.                                                                                                                                    | Native folder-dialog flow and real daemon/backend attachment.                                                          |
| Standalone Settings           | `e1484cc6b` fixes the diagnostic overlay crash and invalid-workspace cleanup; focused tests, typechecks and Desktop build passed.                                                                                                                                                                                                                        | Real Electron open/close and workspace deletion navigation.                                                            |
| DingTalk group ingestion      | `7ac558f9f` and `84d1e7d8f` restore selection, session/revision fences, old-binding repair and late-reply suppression. PostgreSQL-backed race tests and migration 577 upgrade/rollback passed in [33755154457](https://github.com/alexj11324/Cordy/actions/runs/33755154457).                                                                            | Real DingTalk stream/provider test.                                                                                    |
| DingTalk group settings       | `76ad44ff1` restores shared UI, API/schema/query/WS contracts and four locales. Real browser + HTTP fixture verified success → failure preserving the old selection → retry; CI [33758176228](https://github.com/alexj11324/Cordy/actions/runs/33758176228) passed all applicable jobs.                                                                  | Real backend/provider reassignment in Web/Desktop.                                                                     |
| Hosted installation capacity  | Hoplite checkpoint `a75c0591`: migration 576, seven installation admission surfaces, pause/resume worker, work-discovery filters. Later takeover CI includes these tests.                                                                                                                                                                                | Live Cloud entitlement delivery, deployed enablement and pause/resume acceptance.                                      |
| Managed Slack credentials     | `377bb52a9` and `96afd5368` preserve Hoplite's credential work and restore encrypted rotation, health and lifecycle. Replacement CI [33762592403](https://github.com/alexj11324/Cordy/actions/runs/33762592403) passed all applicable checks on `1e54e1d9f`, including PostgreSQL-backed race tests and sqlc.                                            | Real Slack authorization/refresh/revoke and deployed mode acceptance.                                                  |
| Workspace messaging Hub       | `278a2b69b` ports selection, invocation permissions, transactional persistence, pending-run fencing, localized replies and adapter wiring. `284aa95a1` isolates resume state by Agent; `1eb6b8cee` integrates the final Hoplite guard. Backend build and PostgreSQL-backed race tests passed in CI 33768750201; sqlc drift was corrected in `a25efa85a`. | Replacement CI after sqlc synchronization; real provider and setup/UI acceptance.                                      |
| Messaging setup and health UI | Rust-reference frontend has `packages/core/types/messaging.ts` and shared `integration-setup-guide.tsx`; these are absent from the inspected Go tree.                                                                                                                                                                                                    | Port backend setup/runtime projection, shared contracts/UI, installation mode and truthful deployment instructions.    |
| Hosted IM turn quota          | Rust entitlement/task service enforce `im_agent_turns`; no matching contract was found in the inspected Go server/core/views.                                                                                                                                                                                                                            | Per-turn admission, usage accounting/endpoint, UI, failure/rollback semantics and Cloud acceptance.                    |
| Hosted workspace quota        | `hosted_workspace_limit` is an identified migration gate, not implemented/verified by the takeover.                                                                                                                                                                                                                                                      | Trace Rust admission and policy contract, port all creation paths, verify concurrency and hosted/self-hosted behavior. |
| Authentication and delivery   | No current takeover evidence establishes shadcn login → real Clerk → Go session → API completion, or deployed Go backend/build identity.                                                                                                                                                                                                                 | Real browser authentication, native callback, release artifacts, production health/version and provider connectivity.  |

The table does not certify unaudited areas such as guest isolation, capability
leases, work products, Linear, mobile, or all runtime adapters. Route/type/schema
presence is insufficient evidence of behavioral parity.

The frontend deletion inventory also includes the Rust-reference Clerk
provider/adapter, `/sign-in`, `/sign-up`, SSO callback and Google OAuth route
files. The current Go Web login explicitly hides Google except for a Desktop
handoff, and its tests assert that email-only ordinary Web behavior. This needs
source/flow reconciliation, not merely a visual login-card check. Other deleted
files include Desktop development-runtime contracts and issue/chat UI surfaces;
trace any replacement before classifying each as preserved or missing.

## Managed Slack credential acceptance

Implementation entry points:

- `server/internal/integrations/slack/managed_oauth.go`: retain refresh material,
  derive expiry from the exchange clock, reject invalid lifetime or HTTP status.
- `managed_install.go` and `config.go` in the same package: seal both credentials
  with the existing installation key; public configuration remains secret-free.
- `managed_token_worker.go`: immediate boot sweep, five-minute cadence,
  thirty-minute refresh window, bounded provider work and cancellation.
- `server/pkg/db/queries/slack_managed.sql`: list workspace-owned installations
  without an Agent join; fence token writes by the refresh credential and health
  writes by the access credential. Paused/revoked/deleted/BYO installations may
  not receive late writes.
- `server/cmd/server/main.go`, `router.go`, `shutdown.go`: construct, run, cancel
  and join the worker with the server lifecycle.

The tests use fixture credentials and local HTTP servers. The database test
uses its own PostgreSQL records, exercises rotation/readback and stale writes,
and never sweeps other tests' installations. No real Slack account is involved.
No new migration, secret, external service, or production flag was introduced.
The existing managed client credentials and encryption key enable this worker;
full deployment `messaging_mode` parity remains part of the setup/Hub slice.

An older Go-managed installation that lost its refresh token must reconnect
through OAuth. Its missing refresh material cannot be reconstructed from the
access token. Preserve the original encryption key for stored credentials.

## Workspace Hub implementation checkpoint — CI verified

`284aa95a1` isolates Chat execution pointers by producing Agent, including late
completion/cancellation, retired-session clearing and task-history fallback.
Two Agents sharing a runtime may not reuse one another's provider session or
working directory. A daemon-claim regression covers an old task after selection
has changed.

`278a2b69b` adds the shared Hub resolver and transactional selection persistence,
all six provider reply adapters, four-language command copy, managed Slack
`/agents` normalization, and selection-aware `/issue`, `/new` and `/clear`.
The pending-run fence waits for already-firing as well as queued contexts before
switching. The database test exercises real Slack resolvers and shared session
transactions; the Agent-execution boundary is a fixture, not a real Agent CLI.

`1eb6b8cee` preserves Hoplite's concurrent `9df503f44` commit and its unresolved
Agent guard. The Slack conflict was resolved against the Rust contract: retain
group scope and require command identity, rather than converting every unknown
slash command into direct-message Agent input. No history was force-overwritten.

The RED tests in `12a963327` failed in CI
[33763887910](https://github.com/alexj11324/Cordy/actions/runs/33763887910): an
unresolved workspace installation wrote a Chat, and managed `/agents` was ACKed
without entering routing. The implementation above subsequently passed backend
build, database migrations and race tests in CI **33768750201**. SQL artifact
drift from that run is corrected in `a25efa85a`, and replacement CI
**33769723534** passed all applicable jobs. Local JSON formatting and whitespace
checks pass. No local Go/Rust command, live provider, deployment,
credential change or native UI acceptance occurred in this slice.

The earlier Slack credential replacement CI
[33762592403](https://github.com/alexj11324/Cordy/actions/runs/33762592403) on
`1e54e1d9f` passed every applicable job, including sqlc. That result closes the
credential row's replacement-CI gate above, not its real-provider gate and not
this subsequent Hub change.

### Retained acceptance boundaries

Restore the shared engine behavior before presenting workspace-level setup as
complete. The Rust reference resolves identity/membership before selecting the
Agent; `/agents` control commands do not become Agent turns. Persisted selection
must honor current invocation permissions. Slack `/issue` is a separate entry
and must honor that selection too. Switching during a debounced pending turn
must not run the old message under the newly selected Agent.

Agent visibility is not invocation permission: the Go management list lets
workspace admins see other members' private Agents, while the Rust Hub allows
only the sender's own Agents or explicit workspace/member invocation targets.
Reusing the management-list predicate would silently broaden access.

Audit all affected adapters, session persistence, outbound replies, permissions,
events, shared UI and generated SQL. Do not insert a hard-coded/default Agent
to conceal the missing routing stage. Keep ordinary Agent-owned BYO routing
unchanged while adding the workspace-owned path.

## Desktop ordinary-build boundary

`55b539806` restores the Rust-reference split: the public Desktop `build` script
runs only `electron-vite build`. The unchanged `package.mjs` still prepares the
target CLI separately before packaging. A regression first failed because
`build` invoked `bundle-cli`; all 32 script tests now pass, and focused ESLint
passes. The actual `pnpm --filter @patchbay/desktop build` command completed
main, preload and renderer outputs without a Go/Rust invocation. Renderer build
time was 2.34s on this prepared worktree, not a backend cold-build benchmark.
Existing CSS highlight and dynamic-import warnings remain.

This does not complete development-runtime migration: `dev.mjs` still invokes
the CLI bundler, whose timestamp prevents an exact no-op build claim and whose
missing-tool/binary paths still permit old or release CLI fallback. Source-matched
dev runtime preparation, caches, isolated backend/database startup and packaged
artifact acceptance remain open. No installer or production release was run.

## Full migration completion gate

### Connection status terminology

User decision: the pending messaging UI must be titled **Connection status** /
**连接状态** / **接続状態** / **연결 상태**, not "health status" or "健康状态".
This follows the interface-copy rule to name the user-observable behavior
directly. The title describes the bot's connection to its messaging platform,
not the health of the user's computer or guaranteed Agent execution.

Keep desired installation enablement separate from observed connectivity:
`active` alone must not produce an "已连接" label. The connection UI must state
connecting, connected, disconnected or the specific connection problem using
server observations, and show unavailable status when evidence is absent.
Generic service health probes and existing protocol enum values are not renamed
by this presentation decision. This records the agreed terminology; the setup
and connection-status UI migration above is still pending.

### Completion requirements

Before claiming complete, expand the inventory to every production feature and
close each source → contract/schema → Go implementation → UI → runtime chain.
Record intentional exclusions only with explicit user approval. Required CI,
valid review findings and requested real runtime/deployment acceptance must all
be satisfied before the migration stack is merged. Green CI and migration-file
counts alone do not authorize that conclusion.
