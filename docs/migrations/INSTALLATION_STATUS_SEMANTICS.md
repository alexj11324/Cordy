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

Migrations 578–580 preserve installation IDs, credentials, binding rows,
timestamps and quota pauses; change the default/check constraint; and replace
the partial lease index with an installed-state predicate. Concurrent index
creation/deletion each has its own migration file. No FK/cascade is added.
There is no dual-name runtime adapter. Backend and clients must be upgraded
together for this intentional contract change.

### Allowed legacy locations

- Immutable historical SQL 109/124/576 describes the original state.
- Migration 578 up/down and 580 down necessarily read/restore `active` for
  upgrade/rollback. These are database conversion, not shipping dual-name logic.
- `messaging-installed.test.ts` rejects the old value; the migration round-trip
  test and historical migration fixture recreate the old schema deliberately.
- Other domains' active sessions, tasks, Agents and leases are different
  concepts, not installation-status aliases.

Owner: this migration task. Deletion condition for conversion SQL and historical
fixtures: only a separately approved schema-baseline squash that removes the
corresponding old schema/rollback target. No runtime adapter exists or needs a
deletion deadline.

## Verification checkpoint

- Core: 21 focused tests passed, including installed accepted / active rejected.
- Views: 141 focused tests passed across six provider pages, group routes and
  Agent integration views; existing jsdom navigation warning remains.
- Core and Views typechecks passed. Mobile copy tests passed (2 tests);
  Mobile typecheck/lint passed after updating its installation-success alert.
- Backend new/default schema, up/down/up data preservation, query generation,
  provider reporting and all required CI: pending the replacement run.
- Browser/native/provider/deployment acceptance: not yet completed for this
  checkpoint. Unit tests and compiled DTOs do not prove live connectivity.

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
