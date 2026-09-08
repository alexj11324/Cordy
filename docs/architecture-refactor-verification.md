# Architecture refactor verification

Implementation source: the eight-stage architecture plan and the supplemental audit “检查代码库漏洞优化方案” (6a9f3595-15c0-83ea-9ddb-e55c043b09b6).

Verified code: `e4cdf10099b5c15d8b3ba8f466f87a89ce3fa82a`, based on upstream `7b3b9a5fcf`. This document adds evidence only; it does not change executable code.

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
- Terminal migrations are 597–599 on this baseline: one table and two single-statement concurrent unique indexes. Fresh installation and upgrade from upstream 596 succeeded; both indexes are valid.

## Verification results

- Final service, handler, middleware and dbid package race run: **4,615 passing test events, zero failures, two existing skips**. Daemon, Linear, protocol, migration and application packages also passed their affected race runs; the final corrections were rerun in their complete affected packages.
- `go build ./...` passed. sqlc 1.31.1 regenerated 75 files with zero byte differences.
- Complete core tests: **1,946 passed** across 172 files. Complete Mobile tests: **220 passed** across 43 files, plus its startup-script assertions. Neither test run skipped tests.
- Canonical frontend typecheck passed all nine tasks; Mobile typecheck also passed. After upstream #786 integration, another 114 task-view tests and 44 cache-updater tests passed. Realtime source is unchanged by the final backend corrections.
- Relevant complete lint runs reported zero errors; existing warnings remain.
- Independent backend/code and TypeScript reviews approved the final implementation. No unresolved current regression remains in the reviewed scope.

The expanded repository run is **not entirely green on this Mac**: one CLI discovery test detects installed codex/copilot despite its empty fixture environment, and five existing Codex simulated-process tests time out. All six were reproduced with matching assertions on pristine upstream `7b3b9a5fcf`; their production source was unchanged. Optional platform/extension/live-provider tests retain their existing skips. These failures are not counted as passing checks.

## Electron and HTTP acceptance

The dedicated Electron renderer at port 5699 used the task's API at port 18605. The startup script verified the listening process and compiled commit `e4cdf10099` before opening the client.

1. HTTP creation of an issue assigned to the controlled fixture agent persisted exactly one queued execution; Electron displayed the new card without reload.
2. Completion and exact replay returned 200 with identical acknowledgement; contradictory and stale reports returned 409. The database and Electron showed one result comment and one completed execution.
3. An HTTP title edit appeared immediately in the open Electron task.
4. While Electron displayed workspace B, an update to A did not appear in B. Returning to A displayed the new title. Both workspace fixtures were loaded before this switching check.
5. With the channel page initially empty, a channel created by the independent HTTP client appeared immediately. A subsequently posted message also appeared immediately.
6. On the final API, a legacy failure request waited on an exact issue-lock holder without locking the task. After release, first delivery and replay returned 200; the result was failed with one comment and no fenced receipt.

Local evidence is retained under the task's cache directory: `architecture-ui-state.json`, `architecture-outbox-live.json`, `architecture-final-verification-e4cdf10099-summary.json`, the migration logs, and the frontend review log directories. Native screenshots are present in the task conversation. No credentials are included in this document.

## Limits and subsequent upstream change

- Provider execution was controlled for failure injection; no external model, live Linear account or iOS device was used. Mobile validation covered its actual transport code and tests, not a native device session.
- The interval between provider completion and the first successful durable save remains a crash window. This is not a strict exactly-once execution guarantee. Rejected/corrupt records are retained for inspection; storage failure cannot be described as durable success.
- No current production hook was found to return a post-write ErrNoRows, but the modern terminal path still maps that hypothetical hook error to conflict. Keep this error-origin distinction as a follow-up. No current producer creates a task with both issue and chat ownership; the schema permits that combination, so its cross-domain deletion lock order is also a follow-up.
- After verification, upstream #779 (`83df56a36b`) changed Go/frontend package identities and added migration 597. That commit is **not included in this verified baseline**. Integrating it requires applying its identity changes to the newly extracted modules, renumbering these unpublished migrations, and rerunning acceptance. The current stack is not claimed to be merge-ready against that later mainline.
- No merge or deployment was performed. The original checkout's unrelated changes were not staged or committed.
