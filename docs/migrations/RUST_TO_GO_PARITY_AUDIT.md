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
For this work item, the maintained shipping-documentation and dependency-graph
role-semantics inventory is complete below; the word “partial” applies only to
the other product slices in this whole-product table.

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

Evidence snapshot for this responsibility-domain handoff: CI
[`33824678711`](https://github.com/alexj11324/Cordy/actions/runs/33824678711)
passed on `0c71849dc` with the dependency-graph code from `395c5fb41` in its
history. The quota rows below therefore distinguish completed implementation
and CI evidence from the still-open Cloud rollout, deployment, and live-provider
acceptance gates.

Backend compilation, SQL generation and tests run in GitHub Actions. No local
Go/Rust tooling or Docker build is part of this audit. Local frontend checks
and browser fixtures complement CI; neither proves real provider connectivity,
native UI completion, or production deployment.

Later connection/installation checkpoint: durable observer ownership, public
runtime projection and six adapter reports are now implemented. CI
[33779190800](https://github.com/alexj11324/Cordy/actions/runs/33779190800)
passed migration, handler, all six provider packages, sqlc and frontend checks;
its overall failure was a daemon timing-sensitive test, repaired in `4410febcc`
with explicit request checkpoints. Replacement CI
[33781049938](https://github.com/alexj11324/Cordy/actions/runs/33781049938)
passed all applicable jobs on `4410febcc`; later copy changes have their own
PR checks.
See [Installation and connection terminology](INSTALLATION_STATUS_SEMANTICS.md)
for the complete installed-state rename, focused tests and browser evidence.

## Inspected slices

| Slice                         | Current evidence                                                                                                                                                                                                                                                                                                                                         | Remaining acceptance                                                                                                   |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| Onboarding project attachment | `2c3336e46` restores repository URLs, native multi-folder consent bridge, daemon-bound local resources, target-workspace headers and four locales. Focused tests/typechecks/builds and a real-browser fixture passed.                                                                                                                                    | Native folder-dialog flow and real daemon/backend attachment.                                                          |
| Standalone Settings           | `e1484cc6b` fixes the diagnostic overlay crash and invalid-workspace cleanup; focused tests, typechecks and Desktop build passed.                                                                                                                                                                                                                        | Real Electron open/close and workspace deletion navigation.                                                            |
| DingTalk group ingestion      | `7ac558f9f` and `84d1e7d8f` restore selection, session/revision fences, old-binding repair and late-reply suppression. PostgreSQL-backed race tests and migration 577 upgrade/rollback passed in [33755154457](https://github.com/alexj11324/Cordy/actions/runs/33755154457).                                                                            | Real DingTalk stream/provider test.                                                                                    |
| DingTalk group settings       | `76ad44ff1` restores shared UI, API/schema/query/WS contracts and four locales. Real browser + HTTP fixture verified success → failure preserving the old selection → retry; CI [33758176228](https://github.com/alexj11324/Cordy/actions/runs/33758176228) passed all applicable jobs.                                                                  | Real backend/provider reassignment in Web/Desktop.                                                                     |
| Hosted installation capacity  | Hoplite checkpoint `a75c0591`: migration 576, seven installation admission surfaces, pause/resume worker, work-discovery filters. Later takeover CI includes these tests.                                                                                                                                                                                | Live Cloud entitlement delivery, deployed enablement and pause/resume acceptance.                                      |
| Managed Slack credentials     | `377bb52a9` and `96afd5368` preserve Hoplite's credential work and restore encrypted rotation, health and lifecycle. Replacement CI [33762592403](https://github.com/alexj11324/Cordy/actions/runs/33762592403) passed all applicable checks on `1e54e1d9f`, including PostgreSQL-backed race tests and sqlc.                                            | Real Slack authorization/refresh/revoke and deployed mode acceptance.                                                  |
| Workspace messaging Hub | Shared Hub selection, permissions, persistence, pending-run fencing, adapter wiring and per-Agent resume isolation passed CI. Workspace-owned NULL/zero-owner installations now enter supervision while orphan records and managed webhooks remain excluded; final CI 33785381994 passed. | Real provider and setup/UI acceptance. |
| Messaging setup and connection UI | Durable observation ownership/projection, six provider reports, shared client validation, installed-state rename and six settings pages are implemented. Redis-authoritative leases are batch-read and token-matched in the public projection; final CI 33787309411 passed. | Workspace-level setup guide and installation modes; real native/provider and deployed acceptance. |
| Dependency graph role semantics | Go now carries the Rust role contract through `server/internal/handler/dependency_graph.go`: `owner` is a member, `executor` and `candidate_executors` are agent/team targets, `reviewer` accepts member/agent/team, and `runtime_id`/`model_id` are paired. The transaction writes the explicit issue/node columns, and Core/View/Mobile graph consumers read `executor_*` and `candidate_executors`; no graph consumer uses the retired aliases. Focused Go and Core contract tests are present, and CI [`33824678711`](https://github.com/alexj11324/Cordy/actions/runs/33824678711) on `0c71849dc` passed all applicable backend, sqlc, frontend, installer, and production checks for this code. | Real apply/read/realtime acceptance. Historical graph migrations retain the old columns only in the immutable up migration and rename-back down migration listed in the residual inventory. |
| Hosted IM turn quota          | Go implementation is present in `79b2cac95`, `11ae5375b`, `server/internal/channelquota`, `server/internal/service/task.go`, `server/internal/handler/messaging_usage.go`, and the Settings/Core contracts. Managed channel-ingested turns use the Cloud `im_agent_turns` gate, count accepted plus in-flight turns, serialize admission on the workspace row, expose usage/reset data, and bypass self-hosted messaging. The earlier targeted run [`33798936215`](https://github.com/alexj11324/Cordy/actions/runs/33798936215) was cancelled before backend completion, but replacement CI [`33824678711`](https://github.com/alexj11324/Cordy/actions/runs/33824678711) on `0c71849dc` passed the complete applicable backend/frontend/sqlc suite. | Cloud entitlement rollout, deployed enablement, and real provider/deployment acceptance. The implementation and replacement-CI gates are no longer open; its end-to-end shipping gate remains open. |
| Hosted workspace quota        | Go implementation is present in `d1707c566` and `36f1b0724`: `server/internal/handler/workspace_capacity.go` resolves the Cloud `hosted_workspace_limit` gate and `server/internal/handler/workspace.go` applies it to workspace creation and owner promotion, with `server/internal/seatcapacity` serializing ownership decisions. CI [`33803663947`](https://github.com/alexj11324/Cordy/actions/runs/33803663947) first exposed two fixtures sending multiple SQL commands through a prepared statement; `36f1b0724` split those statements, and replacement CI [`33824678711`](https://github.com/alexj11324/Cordy/actions/runs/33824678711) on `0c71849dc` passed the complete applicable backend, sqlc, and frontend suite. | Cloud entitlement rollout, deployed enablement, and live hosted/self-hosted acceptance. The implementation, fixture fix, and replacement-CI gates are closed; concurrency and deployment evidence still need live acceptance. |
| Authentication and delivery   | Go source now contains the Clerk provider/adapter, split shadcn shell, SSO callback and Desktop handoff (`8f4d98b49`, `8cb3f6dbb`). Focused source/tests do not establish browser JavaScript → real Clerk → Go session → frontend API completion, or a deployed Go backend/build identity.                                                                                                                                                                                                                 | Real browser authentication, native callback, release artifacts, production health/version and provider connectivity.  |

The table does not certify unaudited areas such as guest isolation, capability
leases, work products, Linear, mobile, or all runtime adapters. Route/type/schema
presence is insufficient evidence of behavioral parity.

The former frontend deletion inventory is now partly reconciled: `8f4d98b49`
restores the Go Clerk provider/adapter, `/sign-in`, `/sign-up`, SSO callback and
Google handoff route files, and `8cb3f6dbb` restores the split shadcn login
shell. The current blocker is evidence, not source absence: no recorded browser
run proves browser JavaScript → Clerk → `/auth/clerk` → Go session → frontend
API completion, and no deployed identity has been checked. Other issue/chat and
Desktop runtime surfaces have also received replacement commits; each row above
still distinguishes source/CI evidence from native, provider, or deployment
acceptance rather than treating a restored file as a closed user flow.

## Stale open-claim reconciliation

The previous version of this audit was written before the quota and several Hub
follow-up commits landed. The following claims were stale and are corrected
above or below:

- **Quota absence:** both hosted quota rows now have current Go contracts,
  focused tests, and a completed replacement CI run. They remain open only at
  Cloud rollout and hosted/deployed acceptance boundaries; “not implemented”
  is no longer true.
- **Workspace Hub restoration:** the older retained boundary said to restore the
  shared engine before presenting workspace setup as complete. `278a2b69b`,
  `284aa95a1`, `14f2c2322`, and `3e3aad17d` provide the routing, resume-pointer,
  workspace-owned supervision, and Redis projection fixes; CI `33787309411`
  passed the applicable checks. The remaining Hub row is therefore live
  provider/setup/deployment acceptance, not a missing shared-engine port.
- **Login and frontend deletion wording:** Clerk and the split shadcn shell are
  restored in source and covered by focused tests. The authentication row stays
  open because the required browser and deployed-backend path has not been
  observed; a source file or green unit test is not that acceptance.

The following are reviewed but are not stale closures: native onboarding and
Electron acceptance, real DingTalk/Slack/provider flows, deployed messaging
setup, source-matched development-runtime preparation, and production
authentication/deployment identity. They remain real acceptance items because
the audit has no current runtime evidence for them.

## Residual inventory and ownership

The responsibility-domain searches classify the remaining legacy vocabulary as
follows. No current dependency-graph request, response, Core type/schema, graph
view, Mobile graph consumer, SQL query, or generated graph model uses the
retired `assignee_*`/`candidate_assignees` fields.

- **Intentional immutable graph history:**
  `server/migrations/465_dependency_graph_domain.up.sql:33-35` records the
  original columns, and
  `server/migrations/519_dependency_graph_executor_fields.up.sql:2,5,8` plus
  `server/migrations/519_dependency_graph_executor_fields.down.sql:2,5,8`
  record the historical rename in both directions. These files are migration
  history, not current schema or API authorities.
- **Intentional negative test:**
  `server/internal/handler/dependency_graph_test.go:72-73` constructs the word
  in a split string and asserts that the explicit contract does not serialize
  it. It is a focused rejection test, not a live alias.
- **Historical public-site changelog:** the legacy wording in the `changelog`
  entries of `apps/web/features/landing/i18n/en.ts`, `zh.ts`, `ja.ts`, and
  `ko.ts` is immutable release history. Current hero/features/about copy and
  all four maintained analytics use-cases now use owner/executor language.
- **Different project-role vocabulary:**
  `apps/docs/content/docs/projects.zh.mdx:19,51`,
  `apps/docs/content/docs/project-resources.zh.mdx:143`,
  `apps/docs/content/docs/projects.ko.mdx:19,51`, and
  `apps/docs/content/docs/project-resources.ko.mdx:144` use localized “project
  lead” wording. This is a project role, not the issue owner/executor/reviewer
  contract, so it is retained.
- **Outside this responsibility or currently owned elsewhere:**
  `server/pkg/publicapi/v1/openapi.yaml:304-331` is the general public Issue
  schema and still needs the general issue-role owner to coordinate its
  contract update; `packages/core/api/schema.test.ts:427-428` is a tested
  old-server Automation compatibility case; `CLAUDE.md` is the repository's
  compatibility pointer and is not a maintained instruction source. General
  Issue compatibility/admission code, daemon prompt files, and current Mobile
  role/review files are not staged by this work item because they belong to
  the other role/domain owners.

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

The later workspace-owned supervisor and Redis-projection gaps are also closed.
RED CI [33783680469](https://github.com/alexj11324/Cordy/actions/runs/33783680469)
proved NULL/zero-owner installations were omitted; `14f2c2322` restores the
Rust owner predicate while excluding managed webhooks at the store boundary,
and final CI
[33785381994](https://github.com/alexj11324/Cordy/actions/runs/33785381994)
passed. RED CI
[33786632818](https://github.com/alexj11324/Cordy/actions/runs/33786632818)
then proved a valid Redis owner was still projected offline; `3e3aad17d`
batch-reads only authorized lease IDs and requires the Redis owner token to
match the durable observation token. Final CI
[33787309411](https://github.com/alexj11324/Cordy/actions/runs/33787309411)
passed all applicable jobs. These are fixture-backed Redis/PostgreSQL results,
not live provider or production-deployment evidence.

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

The shared engine behavior is now restored in the Go source and covered by the
Hub checkpoint above: the Rust reference resolves identity/membership before
selecting the Agent; `/agents` control commands do not become Agent turns;
persisted selection honors current invocation permissions; Slack `/issue` is a
separate entry and honors that selection; switching during a debounced pending
turn does not run the old message under the newly selected Agent. The remaining
boundary is live provider/setup and deployed acceptance, not a missing shared
engine port.

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

User decision: the messaging UI is titled **Connection status** /
**连接状态** / **接続状態** / **연결 상태**, not "health status" or "健康状态".
This follows the interface-copy rule to name the user-observable behavior
directly. The title describes the bot's connection to its messaging platform,
not the health of the user's computer or guaranteed Agent execution.

Keep installation lifecycle separate from observed connectivity:
`installed` alone must not produce an "已连接" label. The connection UI must state
connecting, connected, disconnected or the specific connection problem using
server observations, and show unavailable status when evidence is absent.
Generic service health probes and existing protocol enum values are not renamed
by this presentation decision. The connection-status settings rows and
installation rename are restored; workspace-level setup guidance and the
remaining runtime acceptance above are still open.

### Completion requirements

Before claiming complete, expand the inventory to every production feature and
close each source → contract/schema → Go implementation → UI → runtime chain.
Record intentional exclusions only with explicit user approval. Required CI,
valid review findings and requested real runtime/deployment acceptance must all
be satisfied before the migration stack is merged. Green CI and migration-file
counts alone do not authorize that conclusion.
