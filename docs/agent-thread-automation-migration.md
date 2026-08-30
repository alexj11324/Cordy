# Agent thread and Automation migration acceptance

This document is the implementation ledger for the unified run surface and
the product-concept migration. It is intentionally written as observable
acceptance, not as a promise that a static screen is equivalent to a live
provider session.

## Observable acceptance

### One interactive Agent thread surface

- Issue runs, independent Agent tasks, history, scheduled Automation runs,
  Side Chat, and direct Agent chat all enter the same interactive Agent thread
  surface and use the same message, queue, tool-event, status, error, handoff,
  and composer components.
- No shipping route, button, dialog, panel, menu item, accessibility label, or
  test exposes a dedicated static inspection shell or equivalent inspection
  product surface. Audit data may remain in the thread as structured events,
  cards, and expandable allowed tool/result/error details.
- A visible historical run is interactive whenever its provider session exists.
  The thread resolves provider, session identity, workspace/issue/Automation
  ownership, permissions, and execution lane before allowing a continuation.
- A missing, expired, deleted, unrecoverable, or unauthorized provider session
  renders an explicit unavailable terminal state with the reason and no
  enabled composer. The client never fabricates a new conversation while
  claiming continuity.
- Continuation requests are authorized, idempotent, lane-aware, and preserve
  the provider session. Concurrent sends have a defined conflict result, and
  provider rejection changes the thread to an honest unavailable/error state.
- Continuation task entries expose the complete submitted user turn as the
  canonical `agent_thread_message` field; the bounded `trigger_summary`
  remains routing metadata, and private task context is never returned whole.

### Automation canonical contract

- Shipping frontend paths, modules, components, types, query/mutation names,
  routes, schemas, SDKs, backend modules/types/services, database objects,
  persisted kind/event/activity values, WebSocket events, permissions,
  configuration, CLI, telemetry, docs, fixtures, tests, and generated output
  use `Automation` as the canonical product concept; Chinese user-visible text
  uses `自动化`.
- Existing production data has a reversible, verifiable upgrade and downgrade
  path. This product has no production users, external durable URLs, or
  rolling deployment requiring a compatibility bridge, so this rename has no
  legacy client/API/event adapter. A future adapter would require a separate
  deployment decision and, if approved, must be isolated, observable, tested,
  owned, and explicitly time-boxed; it may never become a second canonical
  contract.
- The database rename migration is deliberately unnumbered in this branch.
  After the frozen coordination, execution-lane, authorization, and dependency
  graph migrations land, it receives the next contiguous migration number on
  the final stable `main` (currently expected to begin at 430).
- The inert candidate lives under
  `migrations/candidates/automation-rename/`. It covers the complete persisted
  object/data rename, with reversible upgrade and downgrade SQL plus the
  object-count, discriminator, and constraint validation checklist. The
  candidate is not discovered by the migration runner and must not be moved
  into the numbered directory until #629, Dependency Graph, and Work Product
  are stable. Because this product has no legacy adapter, its rolling
  acceptance is an explicit coordinated cutover: drain the old binary, apply
  the rename, then start the canonical binary.

## Real entry points to verify

| Entry point | Required destination |
| --- | --- |
| Issue Agent Working / issue activity | Shared interactive Agent thread |
| Agent activity/history list | Shared interactive Agent thread |
| Mobile Issue Runs active and past rows | Shared interactive Agent thread |
| Automation history and run entry | Shared interactive Agent thread |
| Side Chat and direct Agent chat | Same Agent message/composer tree |
| Provider/session failure | Same thread with explicit unavailable terminal state |

## Explicit deletion list

Delete rather than hide every dedicated run-inspection surface, including its
route, component, trigger, dialog, copy, ARIA/title, fixture, screenshot, and
test. In particular, do not reintroduce a separate event-inspection dialog for
independent or scheduled runs when resolving dependency PRs. The task/run
message and timeline data models may remain when they are used as structured
thread events, persistence, audit, or provider recovery inputs; they are not a
separate user-facing product surface.

## Verification ledger

- Full-tree search has no old canonical product spelling outside this policy
  and immutable historical migration/changelog records; there is no adapter in
  this product migration.
- Generated API/OpenAPI/SDK schemas and WebSocket event registries agree with
  the Automation contract.
- Database upgrade and downgrade are exercised against representative legacy
  data; new-client Automation E2E covers create, trigger, run history, and
  continuation. Since there is no supported old-client compatibility window,
  coordinated cutover is the validation boundary.
- Regression tests cover every entry point, provider-session continuation,
  deleted/expired session, permission denial, idempotency/concurrency lane,
  Automation localization, and absence of legacy run surfaces.
- The final PR records each residual legacy location with reason, owner,
  deletion condition, observed usage, and the exact removal release/window.
