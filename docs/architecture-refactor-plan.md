# Architecture refactor implementation

Source: the approved conversation “检查架构并重构方案” (6a9d5db6-9a5c-83e9-9474-54b2c2c957f6).
Implementation baseline: origin/main at `1ac6b8bc3f5a04ed139826daf1bec5f08770cd0f`.
Integrated upstream baseline: `83df56a36b`, including the Orvilo identity cutover. Terminal migrations use 598–600 because upstream already owns 597.

Preserve the Go modular monolith, PostgreSQL/sqlc, Redis relay, independent daemon, and the existing frontend package and state boundaries. Each phase is a separately reviewable change; a passing build alone does not establish runtime acceptance.

## Delivery order and acceptance

1. **Terminal report acknowledgement.** Bind each report to its task and claim generation, give it a stable identity and digest, and persist acceptance in the existing terminal transaction. Concurrent duplicates must have one durable outcome, conflicting results and stale claims must be rejected, and a failed transaction must not acknowledge success. Existing daemon payloads remain supported. Verify completion/failure, rollback, replay, concurrency, authorization, and context-exhaustion rerouting against PostgreSQL and HTTP handlers.
2. **Durable daemon reports** (depends on 1). Persist complete/fail reports outside collected workspaces before sending. Recover after process restart, use the same identity across retries, and remove only acknowledged entries. Test outages beyond the old retry budget, lost responses, corruption, disk errors, permanent rejection, cancellation, and restart without another provider invocation. Record pending count/oldest age. Server support must ship before new daemon support.
3. **Atomic issue enqueue** (independent of 1–2). Prepare external overlays outside transactions; write issue, labels, uploaded-attachment ownership and actual execution task in one transaction. Recheck executor, workspace and attribution facts before commit. Publish and wake after commit. Preserve backlog, direct-agent offline queueing, blocked runtimes, team leader routing, and deferred media semantics. Verify rollback and committed task existence even when notification fails.
4. **Application assembly and use-case boundaries** (after reliability acceptance). Move assembly/lifecycle ownership out of HTTP routing and narrow the concrete task/coordination dependency cycle. Use the verified issue/run slice and typed business events; avoid wrappers around every generated query. Verify startup/shutdown and affected business entry points.
5. **Daemon execution phases** (after 2). First separate existing lifecycle code mechanically, then use explicit stage inputs/outputs along existing seams. Preserve prepare deadlines, claim identity, task credentials, cancellation, provider/session differences and worktree retention/cleanup. Verify existing lifecycle tests and real task execution.
6. **Linear lease ownership** (independent fault fix, extraction after it). Cancel processing on renewal error or zero updated rows. Fence local writes and completion by claim ownership and generation; preserve FIFO and database-trigger outbox production. Verify competing workers, expiry during work, remote success/local failure and replay. Then move the worker out of the HTTP container and consolidate queries without duplicating event production.
7. **Realtime projections** (independent of backend structure). Validate envelopes and known payloads, safely ignore unknown events, and dispatch scoped domain projections. Preserve batching, reconnection reconciliation, Query ownership and permission-sensitive server refetch. Verify malformed/duplicate/out-of-order events, old-client compatibility, reconnection and workspace changes in Electron.
8. **Configuration and boundary guards** (after 4–7). Consolidate observed configuration duplication and enforce dependency, migration and protocol contracts. Verify fresh installation and upgrade plus the affected desktop workflows. Do not add speculative abstractions or change deployment configuration.

## Runtime and invariants

- Development uses isolated worktrees, databases, ports, build outputs and daemon profiles; existing checkouts remain untouched.
- Use repository Go 1.26 and Node 22 toolchains. Disposable probes belong under a per-user cache directory.
- No new foreign keys; each concurrent index has its own single-statement migration and migration-runner cleanup registration.
- Authorization, capability leases, quotas and attribution remain enforced at all affected entry points.
- Desktop acceptance uses Electron. No UI geometry change is intended; retain existing layouts while testing data, reconnect and workspace transitions.
- Record observed failures, commands, evidence and remaining limitations with each phase. Do not infer performance improvement from file size reduction.

## Progress

- Complete approved prose retrieved; its separate evidence attachment was not exposed by the conversation connector.
- Fresh main confirms post-commit ordinary issue enqueue and Linear renewal loss gaps. Existing terminal transaction hooks and claim fences are available for reuse.
- Independent worktree created; implementation and verification in progress.

### Accepted reliability checkpoints

- Issue enqueue: `f16f71442b` in `codex/architecture-issue-enqueue`. Real PostgreSQL/Redis service and HTTP checks passed (65 top-level tests, no skips). Independent review exposed source/attachment and workspace/attachment lock cycles; both now have deterministic PostgreSQL regression tests. The final focused race run passed after those fixes.
- Linear leases: `216d2066a8` in `codex/architecture-linear-lease`. Real PostgreSQL tests plus a controllable provider passed, including full `TestLinear` with the race detector. Independent review reproduced lease expiry during an unchanged-row lock wait; renewal and local write admission now evaluate expiry after acquiring that lock. Independent replay of both queues returned zero for expired/changed-owner claims and one for a live claim. A real Linear account was not used to manufacture these faults.
- Terminal acknowledgements: concurrent replay, conflicting identity, stale claim, transaction/hook rollback, NUL sanitization, context-exhaustion rerouting, receipt cleanup and migration guards passed with the race detector and no skips. Independent review also added nonblocking issue-lock admission to avoid a newly introduced task/issue deadlock.
- Live terminal check: rebuilt API at `localhost:18605`; created `DEV-1` through Electron, then submitted controlled terminal callbacks through the real HTTP server. First send/replay returned 200 with identical acknowledgement; conflicting payload and stale claim returned 409. PostgreSQL contained one receipt and one result comment. Electron showed one discussion and one completed execution. This exercises result reporting, not a real provider invocation. Evidence: local probe `architecture-terminal-live.json` under the task's temporary cache directory.
- Daemon delivery remains in progress: storage, acknowledgement and restart probes passed, but review found startup orphan recovery ran before replay. A fix protects exact pending task/claim pairs while preserving unrelated orphan recovery; it is undergoing verification before the daemon checkpoint.
