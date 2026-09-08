# Architecture refactor verification

Implementation source: the eight-stage architecture plan and the supplemental audit “检查代码库漏洞优化方案” (6a9f3595-15c0-83ea-9ddb-e55c043b09b6).

Verified tree: `5558d1e1688c388c43c57edb66b087d0bd77ab35`, with executable changes through `457a8386eb`, based on upstream `83df56a36b` including the Orvilo identity cutover. Subsequent documentation changes do not change executable code.

## Audit coverage

| Finding | Implemented behavior | Verification |
| --- | --- | --- |
| F01, F10 | Issue, labels, attachment ownership and executor task commit together; external preparation precedes the transaction. | PostgreSQL rollback, commit visibility, offline/team/backlog/review admission, precise attachment/source/workspace lock races; real HTTP creation yielded one queued task visible in Electron. |
| F02 | Stable fenced reports are persisted outside workspace GC; matching acknowledgement removes them; restart replays reports without invoking the provider. | Real API with separate OS processes for unsent and lost-response cases; each produced one receipt/comment and no retry task. Corruption, permission/I/O failure and protected orphan recovery have regressions. |
| F03 | Both legacy and fenced terminal callbacks commit result comments with task state; post-write errors cannot become legacy idempotent success. | Rollback, no-action/redaction/suppression, duplicate delivery, hook errors and FailTask/Rerun races. Real legacy HTTP waited for an issue writer while leaving the task unlocked, then returned 200 twice with one comment. |
| F04 | Only typed invalid-grant rejection requires reauthorization; temporary refresh failures retain active state and existing backoff. | Provider HTTP classification plus PostgreSQL/fake-provider recovery tests, including 408/429/5xx bodies containing invalid_grant. |
| F05 | Lost or expired leases cancel work and reject stale writes/renewals after acquiring row locks. | Both queues, competing owners, unchanged-row waits, remote success/local failure and replay with race detection. |
| F06 | Web/Desktop and Mobile validate known payloads and independently isolate subscriber errors. | Malformed/nested/overflow payloads, synchronous throws, rejected promises, current Go producers, actual channel transport-to-cache tests. |
| F07 | Missing owner/executor flags stay undefined so cache comparison can infer changes. | Real QueryClient filtered-list regression, explicit-false distinction and Mobile transport preservation. |
| F08 | Catalog coalescing has a 100 ms deadline and retains connection/workspace identity. | Sustained streams, disposal and replacement-generation tests. |
| F09 | A pure Linear project move publishes the changed project and revision. | Real PostgreSQL, fake provider, old/new project projections, duplicate event and no-echo checks. |

## Structure and compatibility

- Application services, worker lifecycle and configuration assembly moved out of HTTP routing. HTTP and background dispatch share the same task service/cache; business creation facts no longer depend on transport payloads.
- Daemon preparation and provider stages have explicit inputs and limited capabilities. The mechanical extraction accounted for all 265 original declarations and 214 functions; 211 bodies were byte-identical, with the remaining changes restricted to reviewed stage bindings. File size is not treated as a performance benchmark.
- Linear synchronization lives in `internal/integrations/linearsync`; its maintained import guard excludes the HTTP container, application service package and HTTP transport.
- Repository guards cover actual guest/auth routing, production scheduler registration, UUID creation, migration ownership/cleanup and frontend event-producer dependencies. CI admits the architecture stack's base branches on both main and Mobile workflows.
- Terminal migrations are 598–600: one table and two single-statement concurrent unique indexes, after upstream 597. Fresh installation and an upgrade from the 596 schema through the Orvilo identity migration succeeded. Both receipt indexes are ready, valid and unique; live database checks reject duplicate report identities, duplicate task/claim pairs and nonterminal outcomes.

## Verification results

- Complete service race run: **709 passing test events**. Final handler, application, middleware and dbid race runs passed **3,772 / 480 / 112 / 23** test events respectively; handler retained two existing skips. Counts include subtests.
- Complete Linear provider and synchronization module race runs passed. Daemon storage, execenv, repocache, processtree and protocol packages passed. Migration packages passed 55 test events, with one existing pg_bigm availability skip.
- The complete daemon package run passed 1,489 test events and failed one 50 ms watchdog fixture (`TestExecuteAndDrain_IdleWatchdog_DoesNotFireDuringInFlightToolCall`), with one existing skip. One exact isolated race run passed. The test, its tool-call fixture and watchdog function bodies match upstream `83df56a36b`; the fixture uses parallel scheduling and a 50 ms timer. The failing run has no event-order trace, so the exact cause remains unproved and the full-run failure is retained rather than counted as a pass.
- `go build ./...` passed. sqlc 1.31.1 regenerated **78 files with zero byte differences** in an immutable archive.
- Complete core tests: **1,946 passed** across 172 files. Complete Mobile tests: **220 passed** across 43 files, plus its startup-script assertions. Neither test run skipped tests.
- Canonical frontend typecheck passed all nine tasks without cached results; Mobile typecheck also passed. Relevant complete lint runs reported zero errors. The 2 / 15 / 27 core / Mobile / Views warnings are in files unchanged by this work relative to upstream.
- Independent backend/code and TypeScript reviews approved the integration. The daemon review accounted for 1,742 production declarations and separately checked imports, tests and platform files. The realtime review compared all 20 changed files against the earlier approved implementation and upstream identity changes.
- CI triage contract tests passed. Both workflow base-branch filters are included from the first stack layer so each dependent PR can trigger its quality gates.

An earlier expanded repository run on the pre-cutover `7b3b9a5fcf` baseline found one CLI discovery fixture affected by installed executables and five Codex simulated-process timeouts. All six reproduced on pristine upstream at that baseline. Those historical failures are not counted as passing current checks, and the current repository is not described as fully green based on affected-package results alone.

## Electron and HTTP acceptance

The dedicated Orvilo Electron renderer at port 5699 used the task's API at port 18605 and the fresh `orvilo_cordy_architecture_final_525` database. The new Orvilo daemon also started from this worktree with the dedicated local API profile. The startup script verified the listening process and compiled commit `5558d1e168` before opening the client.

1. HTTP creation of an issue assigned to the controlled fixture agent persisted exactly one queued execution; Electron displayed the new card without reload.
2. Completion and exact replay returned 200 with identical acknowledgement; contradictory and stale reports returned 409. The database and Electron showed one result comment and one completed execution.
3. An HTTP title edit appeared immediately in the open Electron task.
4. While Electron displayed workspace B, an update to A did not appear in B. Returning to A displayed the new title. Both workspace fixtures were loaded before this switching check.
5. With the channel page initially empty, a channel created by the independent HTTP client appeared immediately. A subsequently posted message also appeared immediately.
6. On the final API, a legacy failure request waited on an exact issue-lock holder without locking the task. After release, first delivery and replay returned 200; the result was failed with one comment and no fenced receipt.

Current local evidence is retained under the task's cache directory: `architecture-identity-ui-state.json`, `architecture-identity-67800c4745/`, `architecture-go-final.mbeofJ/`, `realtime-final-5558d1e168-review/` and the independent cutover review directories. Native screenshots are present in the task conversation. The earlier `architecture-outbox-live.json` retains the separate-process unsent/lost-response replay acceptance before the identity cutover; the reviewed store/sender and report bodies are unchanged. No credentials are included in this document.

## Identity integration and limits

- Provider execution was controlled for failure injection; no external model, live Linear account or iOS device was used. Mobile validation covered its actual transport code and tests, not a native device session.
- The interval between provider completion and the first successful durable save remains a crash window. This is not a strict exactly-once execution guarantee. Rejected/corrupt records are retained for inspection; storage failure cannot be described as durable success.
- No current production hook was found to return a post-write ErrNoRows, but the modern terminal path still maps that hypothetical hook error to conflict. Keep this error-origin distinction as a follow-up. No current producer creates a task with both issue and chat ownership; the schema permits that combination, so its cross-domain deletion lock order is also a follow-up.
- Upstream #779 is included. Newly extracted code uses Orvilo module/package names, login storage, request headers, Linear columns, marker helpers, source values and remote-apply GUCs. The GUC names were checked against the actual 597 database triggers. Upstream cleanup support for historical task directories is preserved. The previous verified branch and databases remain available as separate checkpoints.
- No merge or deployment was performed. The original checkout's unrelated changes were not staged or committed.
