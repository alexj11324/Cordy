# Cordy Go→Rust Refactor — Serial Execution TODO

> Owner: the current coordination thread only. No new Codex threads, no delegated
> PR fixes, and no parallel implementation or Cargo execution.

## Operating contract

- [ ] Process exactly one work item at a time; keep all other PRs frozen.
- [ ] Before every mutation, record the exact branch, head, base, tree, and PR.
- [ ] Use non-rewriting commits only; never force-push, amend, or reuse a stale gate.
- [ ] Every PR must be Ready (`isDraft=false`) before review or gate.
- [ ] Request at most one exact-head Codex review per head.
- [ ] Run one shared Cargo gate at a time with `CARGO_INCREMENTAL=0`,
      `CARGO_BUILD_JOBS=1`, and `--locked`; record baseline-only failures separately.
- [ ] Merge only with GraphQL `expectedHeadOid`; verify merge parents/tree/ref afterward.
- [ ] Do not spend implementation cycles on P2/style-only findings unless they are
      changed-code compile errors or the user explicitly reprioritizes them.
- [ ] Update this file immediately after each state transition and commit it with the
      corresponding version-management change.
- [x] Installed `/home/ubuntu/.codex/skills/refactor/SKILL.md`; apply its behavior-
      preserving, test-first, one-smell-at-a-time process to every refactor slice.

## Current baseline (2026-08-24)

- Integration remote: `ef7e3ba9da342ca4deb4bd12d21a3c60a170c713` (after #89 merge).
- Current local integration head: `8438d39bc1b28a561f846e83590ef1771a20dc61`.
- Local-only commits requiring audit/version decision: `cbb0d3aa` through `8438d39b`
  (nine commits, common ancestor `881da969`; local is `9` ahead / `212` behind remote).
- These local commits touch roughly 170 files and include large deletions; they must be
  reconciled onto the live integration tree before any push. Never push this local branch
  blindly and never reset it destructively.
- Root worktree has pre-existing untracked `.tmp/` and `.worktrees/`; do not delete.
- No root `TODO.md` existed before this file.

## Phase 0 — inventory and reconcile local history

- [ ] Read-only audit each local-only commit against the remote integration tree; preserve
      the commits as evidence until each change is classified.
- [ ] Reconcile the valid subset on top of the live integration without rewriting the
      original local commits; use new atomic commits or an existing PR branch as appropriate.
- [ ] Classify each local commit as `already represented`, `valid pending integration`,
      `needs review`, or `unsafe/obsolete`; do not push blindly.
- [ ] Audit all currently open PRs and write their exact head/base/tree/candidate here.
- [ ] Mark PRs already represented by integration as `SUPERSEDED` in this file; keep
      them open unless the user explicitly asks to close them.
- [ ] Establish one queue ordered by dependency first, then P0/P1 risk, then PR number.

## Phase 1 — current daemon chain

- [x] #89 fixture P1 resolved by current head `2a8b42d6`; `createVerificationCodeForTest`
      now uses `server/internal/testutil` fixture insertion.
- [x] #89 exact head `2b7d31e5f20d08c90f22c99c19ff6a0b04637a39`, tree
      `83c10acd88cb411d00bd77c8c5778873c52b9d54`, base
      `8b44b1ef7b54771a19f2a658a047628d9174274c`, candidate
      `e3ef39d5f7c358d3cd219c2732e87b638856fa98`; exact-head review comment
      `5395048184` returned no major issues.
- [x] #89 serial gate passed under policy: metadata/check/server PASS; daemon tests
      `398/408` with 10 unchanged sandbox/filesystem failures; handler tests `369/375`
      with 6 unchanged environment/baseline failures; strict Clippy only the 5 known
      unchanged `heartbeat_scheduler`/`runtime_sweeper` style lints. No changed-code
      compile, test, security, or lifecycle failure. Lock released.
- [x] #89 merged with GraphQL expected head `2b7d31e5`; merge `ef7e3ba9`, parents
      `8b44b1ef + 2b7d31e5`, tree `83c10acd`; integration ref verified.
- [ ] #113 restacked non-rewriting in clean `/home/ubuntu/Cordy/.worktrees/cord-113-restack`:
      head `709838df71c1f6ba05651722b12c1367543c7671`, tree
      `076a681b57b79d0a7e37da9bf9f85299a9ec6677`, parents `fb806b0f + ef7e3ba9`,
      PR base now `2b7d31e5`, candidate `3dcffd6c169ad2772935955b868339fe1cf36de6`,
      Ready/CLEAN, exact-head review requested at `5395491572`.
- [ ] #113 review found one P1 in task-context attachment upload; fixed and pushed as
      `170d7c21ec0a543fa3bb36793f2e661d2f92b58c` (parent `709838df`):
      `run_attachment_upload` now enforces workdir containment before reading the file,
      with regression coverage for an outside absolute path. New candidate is
      `006debb907db00660735da2c9eef2da917a03f0d`; exact-head review requested once at
      comment `5395662140`; gate remains pending review. Previous review/gate is stale.
- [ ] #113 gate remains pending review; original worktree
      `.worktrees/cord-22-daemon-production-assembly` has unrelated uncommitted CLI
      edits and must not be reset or used for the gate. Restacked tree has passed
      `git diff --check` and changed-Rust `rustfmt --check`.
- [ ] #114: restack only after #113 merge; review, gate, expected-head merge.
- [ ] #126 → #127 → #128 → #129: process strictly in that order after daemon base moves.

## Phase 2 — channel/runtime chains

- [ ] #109 → #110 → #111 → #112 (Slack stack).
- [ ] #101 → #104 (Telegram verdict/timeout stack).
- [ ] #102 → #103 (install ownership stack).
- [ ] Process #96, #97, #98, #99, #100, #105, #106, #115, #116, #118, #124
      one by one after each exact-base refresh.
- [ ] For every channel item, verify cancellation, timeout, reconnect, ownership,
      media/file lifecycle, error mapping, and production assembly against Go.

## Phase 3 — CLI, agent, and remaining production surfaces

- [ ] Process #91 and #93 only after their daemon/API dependencies are current.
- [ ] Implement and verify missing CLI contracts: login, setup, daemon foreground,
      update, probe-runtimes, and disk-usage.
- [ ] Audit Go-only background workers, schedulers, reconcilers, event side effects,
      Redis behavior, metrics, and shutdown lifecycle; implement each missing Rust
      production path in the current thread.

## Phase 4 — S8 route and API parity

- [ ] Audit #66→#87 bottom-up against the live integration tree.
- [ ] Mark patch-equivalent/ancestor PRs `SUPERSEDED`; only implement missing behavior.
- [ ] Verify auth, permission, transaction, malformed-response, and response-wire parity
      for every remaining Go route.

## Per-item gate checklist

- [ ] Exact remote head/base/tree/candidate captured.
- [ ] Required non-rewriting restack completed.
- [ ] Scoped rustfmt, `git diff --check`, and offline locked metadata pass.
- [ ] One exact-head review returns no blocking P0/P1.
- [ ] Risk-matched check/tests/strict Clippy run in the shared serial gate.
- [ ] Lock owner/heartbeat/start/head removed and worktree verified clean.
- [ ] Expected-head merge completed and integration parent/tree/ref verified.
- [ ] This TODO entry updated with commit, review, gate, and merge evidence.

## Final parity acceptance

- [ ] Every Go production surface is `VERIFIED`, `MERGED`, or explicitly documented as
      an intentional non-production/deferred surface with evidence.
- [ ] No unresolved P0/P1 security, data, concurrency, lifecycle, or compile blocker.
- [ ] Every open PR is `MERGED`, `SUPERSEDED`, or has a recorded external blocker.
- [ ] Final integration workspace gate passes with no changed-code failures.
- [ ] Final Go/Rust parity report and complete merge ancestry are recorded here.
