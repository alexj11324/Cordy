# Installation and connection terminology

User decision, 2026-09-03: use `installed`, not `active`, for a retained
messaging installation. Use **Connection status / 连接状态 / 接続状態 / 연결 상태**
for provider-observed connectivity. A saved installation is not evidence that
the process is running or the remote platform accepted a connection.

## Contract and migration scope

| Concept | Canonical representation | What it proves |
| --- | --- | --- |
| Installed configuration | `channel_installation.status = installed` | Installation exists and has not been revoked |
| Revoked installation | `status = revoked` | Installation remains for history, but cannot receive/send work |
| Host capacity pause | `hosted_paused_at` | Installed configuration is preserved but work is paused |
| Eligible for a connection attempt | `ListConnectable…` queries | Installed, unpaused, and owner checks pass; not proof of connectivity |
| Confirmed connection | `runtime.state = healthy` plus valid observation ownership | Platform handshake/poll succeeded; the public projection checks lease/observation validity |

The rename covers the six provider adapters, shared routing state
(`ResolvedInstallation.Installed`), admission/capacity counts, query names,
public DTO projection, SQL sources/generated output, shared client types,
Web/Desktop management gates, Mobile WeCom, fixtures, and status copy.
Actual running task/lease/session activity remains `active` where appropriate.
WeCom's revoked-installation skip metric now uses `installation_revoked`.

The public HTTP contract is additive so an installed Desktop client can span a
server rollout. `installation_status` is the canonical lifecycle field. The
legacy `status` field continues to project `installed` as `active`; current
clients prefer `installation_status` and normalize an older server's lone
`status = active` to `installed`. This adapter does not infer connectivity:
only the separate `runtime` projection can produce a connected state.

Migrations 578–580 preserve installation IDs, credentials, binding rows,
timestamps and quota pauses; change the default/check constraint; and replace
the partial lease index with an installed-state predicate. Concurrent index
creation/deletion each has its own migration file. No FK/cascade is added.
The database and internal Go/TypeScript models have no dual-name source of
truth. Only the isolated public-wire adapter above retains the legacy spelling.

### Allowed legacy locations

- Immutable historical SQL 109/124/576 describes the original state.
- Migration 578 up/down and 580 down necessarily read/restore `active` for
  upgrade/rollback. These are database conversion, not shipping dual-name logic.
- `messaging-installed.test.ts` rejects the old value; the migration round-trip
  test and historical migration fixture recreate the old schema deliberately.
- Public JSON `status = active` is a legacy compatibility projection for
  installed Desktop clients. Delete it only after the minimum supported
  Desktop version reads `installation_status`; owner: Desktop/API maintainers.
- Other domains' active sessions, tasks, Agents and leases are different
  concepts, not installation-status aliases.

Owner: this migration task. Deletion condition for conversion SQL and historical
fixtures: only a separately approved schema-baseline squash that removes the
corresponding old schema/rollback target. The public-wire adapter has the
separate Desktop-version deletion condition listed above.

## Verification checkpoint

- Core: 21 focused tests passed, including installed accepted / active rejected.
- Views: 146 focused tests passed across six provider pages, group routes and
  Agent integration views; existing jsdom navigation warning remains.
- Core and Views typechecks passed. Mobile copy tests passed (2 tests);
  Mobile typecheck/lint passed after updating its installation-success alert.
- Backend new/default schema, up/down/up data preservation, query generation
  and all six provider-reporting packages passed in CI
  [33779190800](https://github.com/alexj11324/Cordy/actions/runs/33779190800).
  The overall run failed a separate daemon preparation-timeout test: its 150ms
  timer could expire before the expected request checkpoint. `4410febcc`
  separates the real deadline assertion from checkpoint-triggered cancellation
  and removes sleep-based lease-count polling. Replacement CI
  [33781049938](https://github.com/alexj11324/Cordy/actions/runs/33781049938)
  passed all applicable jobs on `4410febcc`, including the full backend race
  suite. Subsequent frontend/copy changes require their own PR checks.
- The ordinary Desktop build passed without invoking backend tooling; existing
  CSS highlight and dynamic-import warnings remain.
- Real in-app browser, real shared components/API client, local HTTP fixtures:
  all six provider pages exercised starting/healthy/error/paused/offline,
  missing observations, unknown states, and healthy-to-query-error transitions.
  None claimed connectivity without a usable observation or exposed the fixture
  diagnostic sentinel. Offline/error/paused installations retained management.
- On a failed list request, Slack/Lark/DingTalk/WeCom preserve cached records
  but clear their connection confirmation; Telegram/Weixin retain their existing
  explicit list-error screen. Fresh Slack errors expose a working Retry action.
  Slack and WeCom disconnect confirmations opened and were cancelled; WeCom's
  confirmation initially focused Cancel. Widths 320/768/1024/1440 had no
  horizontal overflow for the shared status row, including quota-pause copy.
- Native apps, real provider credentials/traffic and production deployment were
  not exercised. Browser fixtures and compiled DTOs do not prove those paths.
- Workspace-owned NULL/zero-owner installations now enter the connection
  Supervisor, while non-zero missing-Agent orphans and managed Slack webhooks
  remain excluded. Redis-authoritative deployments batch-read only authorized
  lease IDs and require the live owner token to match the durable observation
  generation. RED runs 33783680469/33786632818 reproduced both omissions;
  final runs 33785381994/33787309411 passed all applicable jobs.

### Additional installation-copy corrections

Installed lists no longer say “Connected bots”. Installation completion toasts
and the Lark/Weixin completion screen now confirm installation/account addition,
not that the bot is online. All four locales have regression assertions for
these distinctions. Unused fixed “Connected to…” badge labels were removed;
the shared observation-based component supplies the current label instead.

## Adjacent semantic audit (read-only)

Subagent findings were checked against the current source. These are follow-ups,
not additional changes silently included in the installation rename.

| Finding | Source evidence | More accurate wording/behavior |
| --- | --- | --- |
| Linear `connected: true` means a non-revoked record exists, including reauthorization-required records | `server/internal/handler/linear.go`: GetLinearConnection; `linear_worker.go`: RefreshToken failure | Separate record existence from authorization; e.g. `has_connection` plus explicit authorization state |
| Linear labels active authorization as healthy even when sync failed | `packages/views/settings/components/linear-tab.tsx`: connectionLabel; worker updates last_error without changing active | “Authorized” for authorization; expose synchronization failure separately |
| Linear “projects synced” counts enabled bindings, not completed imports | same UI: activeBindingCount and projects_synced; binding save precedes import enqueue | “Sync enabled for N projects” |
| All deferred tasks are labeled retrying, including first-run attachment waits | `task-status-pill.tsx`: deferred branch; `service/task.go`: CreateChatTask.FireAt = mediaPendingUntil | Distinguish waiting for attachments, waiting for retry, and generic scheduled execution |
| Agent workload can show idle while deferred tasks remain | `queries/agent.sql`: snapshot excludes deferred; `core/agents/derive-presence.ts`: workload counters | Include waiting work without pretending it is running |

The last item depends on the intended meaning of “idle”: it is accurate for
“not executing right now” but incomplete if the UI promises “no pending work”.
Keep execution and pending-work indicators separate when addressing it.
