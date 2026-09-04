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
This document is an evidence-backed responsibility inventory, not a completed
whole-product migration certification or an estimate of all remaining work. The historical
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

Current responsibility-domain evidence: replacement CI
[`33831259464`](https://github.com/alexj11324/Cordy/actions/runs/33831259464)
passed on `a1037b624`, including migrations, SQL generation, the race-enabled Go
test wrapper, frontend checks and platform checks. The backend log reports the
handler, patchbay CLI and server packages passing; the graph acceptance tests
are included in those packages. Mobile Verify
[`33831259471`](https://github.com/alexj11324/Cordy/actions/runs/33831259471)
also passed on the same SHA. This is the replacement evidence for the graph
response, CLI and realtime changes. It closes the source/contract/test/CI gate
for this responsibility block; no production deployment or deployed
apply/read/WebSocket observation has occurred.

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
| Dependency graph role semantics | Go now carries the Rust role contract through `server/internal/handler/dependency_graph.go`: `owner` is a member, `executor` and `candidate_executors` are agent/team targets, `reviewer` accepts member/agent/team, and `runtime_id`/`model_id` are paired. Apply writes those explicit issue/node columns and acceptance criteria atomically; read returns Rust-shaped `parent`, `children`, `nodes`, `edges`, derived `waves`, temp-id edge endpoints, and fail-closed readiness. `patchbay issue dependency-graph get/apply` forwards the typed plan and idempotency header. Core/View/Mobile consumers use `executor_*`, `candidate_executors`, readiness, and `dependency_graph:updated`; no current graph consumer uses the retired aliases. `TestApplyDependencyGraphRoundTripsRolesAndRealtime`, `TestListDependencyGraphsReturnsNestedResponses`, the CLI request/output tests, `TestPostJSONWithHeaderPreservesClientContext`, and `TestRegisterListeners_DependencyGraphUpdatedBroadcastsWorkspaceFrame` are the focused acceptance set. Replacement CI [`33831259464`](https://github.com/alexj11324/Cordy/actions/runs/33831259464) passed on `a1037b624`, with Mobile Verify [`33831259471`](https://github.com/alexj11324/Cordy/actions/runs/33831259471) also green. | Source/contract/test/CI acceptance is closed. A deployed Go service still needs an actual apply → read-by-parent/read-by-plan → WebSocket refresh observation. No local Go test is permitted by repository rules. Historical graph migrations retain the old columns only in the immutable up migration and rename-back down migration listed in the residual inventory. |
| Hosted IM turn quota          | Go implementation is present in `79b2cac95`, `11ae5375b`, `server/internal/channelquota`, `server/internal/service/task.go`, `server/internal/handler/messaging_usage.go`, and the Settings/Core contracts. Managed channel-ingested turns use the Cloud `im_agent_turns` gate, count accepted plus in-flight turns, serialize admission on the workspace row, expose usage/reset data, and bypass self-hosted messaging. The earlier targeted run [`33798936215`](https://github.com/alexj11324/Cordy/actions/runs/33798936215) was cancelled before backend completion, but replacement CI [`33824678711`](https://github.com/alexj11324/Cordy/actions/runs/33824678711) on `0c71849dc` passed the complete applicable backend/frontend/sqlc suite. | Cloud entitlement rollout, deployed enablement, and real provider/deployment acceptance. The implementation and replacement-CI gates are no longer open; its end-to-end shipping gate remains open. |
| Hosted workspace quota        | Go implementation is present in `d1707c566` and `36f1b0724`: `server/internal/handler/workspace_capacity.go` resolves the Cloud `hosted_workspace_limit` gate and `server/internal/handler/workspace.go` applies it to workspace creation and owner promotion, with `server/internal/seatcapacity` serializing ownership decisions. CI [`33803663947`](https://github.com/alexj11324/Cordy/actions/runs/33803663947) first exposed two fixtures sending multiple SQL commands through a prepared statement; `36f1b0724` split those statements, and replacement CI [`33824678711`](https://github.com/alexj11324/Cordy/actions/runs/33824678711) on `0c71849dc` passed the complete applicable backend, sqlc, and frontend suite. | Cloud entitlement rollout, deployed enablement, and live hosted/self-hosted acceptance. The implementation, fixture fix, and replacement-CI gates are closed; concurrency and deployment evidence still need live acceptance. |
| Authentication and delivery   | Go source now contains the Clerk provider/adapter, split shadcn shell, SSO callback and Desktop handoff (`8f4d98b49`, `8cb3f6dbb`). Focused source/tests do not establish browser JavaScript → real Clerk → Go session → frontend API completion, or a deployed Go backend/build identity.                                                                                                                                                                                                                 | Real browser authentication, native callback, release artifacts, production health/version and provider connectivity.  |

The table does not certify unaudited areas such as guest isolation, capability
leases, work products, Linear, mobile, or all runtime adapters. Route/type/schema
presence is insufficient evidence of behavioral parity.

## Complete responsibility-domain inventory and acceptance

The re-audit searched the whole checkout, excluding only `.git`, dependency and
build caches, with:

```bash
rg -l -i 'dependency[_ -]?graph|dependencygraph' . \
  --glob '!**/.git/**' --glob '!**/node_modules/**' \
  --glob '!**/target/**' --glob '!**/dist/**' --glob '!**/.next/**'
```

The current Go checkout result is 90 paths. The Rust authority was inspected
read-only in the frozen primary checkout at
`/Users/alexjiang/Desktop/vibe/Cordy/server-rs/crates/{patchbay-handler,patchbay-service,patchbay-db,patchbay-protocol,patchbay-cli}`;
it is intentionally absent from this Go worktree and is not counted in the 90.
The classified inventory covers: Go routes and migration runner under
`server/cmd`, Go handler and tests under
`server/internal/handler`, CLI and realtime forwarding under
`server/cmd/patchbay` and `server/cmd/server`, SQL queries and generated DB
artifacts under `server/pkg/db/{queries,generated}` plus every graph migration;
Core schemas/types/queries and tests; shared Web/Desktop views and locale
contracts; Mobile API, query, realtime, route, and view consumers; and the
maintained CLI/issue docs plus this audit and the archived W4–W9 record.

The executable acceptance sequence is now explicit: (1) apply a two-node plan
with member owner/reviewer and an agent candidate, then assert atomic child
issue/node/edge persistence, acceptance criteria, root `todo`, dependent
`blocked`, and derived waves; (2) read the same graph by parent and by plan ID
and assert parent/children, temp-id edge endpoints, readiness counts, and
`unlock_condition`; (3) replay the same idempotency key and assert one plan/two
children plus a replay graph event; (4) pass that event through the server
listener and assert a workspace WebSocket frame; (5) run Core/View/docs checks.
The first four steps are represented by the named Go tests in the graph row and
were executed by GitHub Actions because local Go is prohibited. CI
`33831259464` passed the race-enabled backend suite, including the handler,
patchbay CLI and server packages containing those tests. A deployed Go instance
must still be exercised for the same HTTP and WebSocket sequence; until that
deployment observation is attached, the runtime/deployment gate remains open.

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
- **Dependency graph response/CLI/realtime wording:** the earlier graph row
  stopped at explicit role serialization and incorrectly left the reader to
  infer that apply/read/realtime were still absent. The Go handler now has the
  Rust response material and publishes `dependency_graph:updated`; the Go CLI
  now exposes the Rust `get`/`apply` commands. This audit replaces the stale
  wording with named handler, CLI, frontend, and realtime acceptance tests.
  Replacement CI [`33831259464`](https://github.com/alexj11324/Cordy/actions/runs/33831259464)
  passed the graph handler, CLI and realtime packages. The source/contract/test/
  CI gate is therefore closed; a deployed WebSocket observation remains open.

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
- **Current Rust-aligned marketing demo:**
  `apps/web/features/landing/components/features-section.tsx:125-381` keeps a
  product-facing `Assignee` demo picker because the corresponding current Rust
  tree still contains this landing section. It is not a graph API field or a
  persisted role alias; the section is retained rather than deleted during the
  migration. The immutable release-history entries in the four landing locale
  files may also contain old wording and are not current instructions.
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

## Workspace issue category policy

This responsibility block is now implemented end to end from the frozen Rust
authority. The source contract is `server-rs/crates/patchbay-handler/src/issue_status.rs`
(`GET /api/issue-category-policies` and
`PUT /api/issue-category-policies/{category}`) plus
`server-rs/crates/patchbay-service/src/category_policy.rs` and its typed DB
query. The Go port is intentionally limited to the current migration checkout;
the Rust source remains read-only.

| Contract concern | Rust authority | Go implementation/evidence |
| --- | --- | --- |
| Route and workspace scope | Issue-status router; workspace context is supplied by the member guard | `server/cmd/server/router.go` mounts both paths inside `RequireWorkspaceMember`; `server/internal/handler/issue_category_policy.go` repeats the direct-handler membership check and scopes every query by workspace UUID |
| Read/write permissions | GET is member-only; PUT requires `owner` or `admin` | `ListIssueCategoryPolicies` uses `requireWorkspaceMember`; `UpdateIssueCategoryPolicy` uses `requireWorkspaceRole` |
| Input contract | Only `in_progress` and `in_review`; execution is required; review requires a reviewer; blank optional strings mean NULL | `updateIssueCategoryPolicyRequest`, category checks, UUID parsing, required checks, and malformed/trailing JSON rejection preserve the Rust error messages |
| Agent safety | Agent must be a workspace-owned `kind='user'` agent, unarchived, with a runtime; execution/review IDs must differ | `GetAgentInWorkspace` is called through `qtx` inside the write transaction; `ArchivedAt` and `RuntimeID` are checked before upsert |
| Atomicity and errors | Agent validation and upsert share a transaction; DB/commit failures are 500; policy validation is 400 | `server/internal/handler/issue_category_policy.go` begins/rolls back/commits around all validation writes and maps the Rust messages |
| Event | Publish `issue_category_policy:changed` only after commit, workspace/member actor, `{category}` payload | `server/pkg/protocol/events.go` and the post-commit `h.publish` call; `packages/core/types/events.ts` carries the event/payload type |
| Storage and teardown | Application-owned agent relationships; workspace/category uniqueness; workspace deletion explicitly removes policy rows | Existing Go migrations `server/migrations/521_issue_category_policy.*`, concurrent index `522_issue_category_policy_workspace_category.*`, generated model/query, and `workspace_delete.sql` are unchanged and already match the Rust schema/cleanup |

The generated/query chain is complete in `server/pkg/db/queries/workspace_issue_category_policy.sql`
and `server/pkg/db/generated/workspace_issue_category_policy.sql.go`. The shared
frontend transport contract is complete in `packages/core/types/issue-status.ts`,
`packages/core/api/schemas.ts`, and `packages/core/api/client.ts`; its focused
malformed-response coverage is in
`packages/core/api/issue-category-policy.test.ts`.

Consumer inventory was searched across `packages/`, `apps/`, `contracts/`, and
the internal/public API surfaces. There is no current Web, Desktop, or Mobile
settings page, query hook, mutation hook, or i18n key for issue-category
policies, and the Rust tree has no corresponding product entry beyond the
server dependency-admission/review-handoff service. Therefore no UI or locale
file was fabricated: the only current frontend consumer is the typed API
client contract, while `issue_category_policy:changed` is typed for future
cache consumers. The existing Go/Rust dependency-graph reads of the policy are
preserved but deliberately untouched because dependency graph is outside this
responsibility's write scope. `contracts/` contains auth-broker contracts and
`server/pkg/publicapi/v1/openapi.yaml` is the plugin-only public API, so neither
is a valid home for this internal workspace endpoint.

Focused Go handler tests cover workspace isolation, membership and owner/admin
permissions, category/UUID/required-field errors, malformed JSON, agent
ownership/archive/runtime checks, same-agent rejection, committed persistence,
and the post-commit event. Per repository policy, no local Go, Rust/Cargo, or
Docker test/build command was run; generated SQL and the diff were statically
reviewed. GitHub CI and deployed runtime apply/read/WebSocket observation remain
pending for this commit.

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
