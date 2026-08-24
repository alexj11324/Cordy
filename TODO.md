# Cordy Go→Rust Refactor — Serial Execution TODO

> Owner: the current coordination thread only. No new Codex threads or parallel
> implementation/Cargo execution. Serial subagents may implement or review one
> bounded fix when explicitly assigned; the coordinator owns ordering, exact-head
> evidence, gates, TODO transitions, version management, and merges.

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

- Integration remote: `7ac42ed089895a88b165ca1e83734d37429e9286` (after #114 merge).
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
- [x] #113 restacked non-rewriting in clean `/home/ubuntu/Cordy/.worktrees/cord-113-restack`:
      head `709838df71c1f6ba05651722b12c1367543c7671`, tree
      `076a681b57b79d0a7e37da9bf9f85299a9ec6677`, parents `fb806b0f + ef7e3ba9`,
      PR base now `2b7d31e5`, candidate `3dcffd6c169ad2772935955b868339fe1cf36de6`,
      Ready/CLEAN, exact-head review requested at `5395491572`.
- [x] #113 review found one P1 in task-context attachment upload; fixed and pushed as
      `170d7c21ec0a543fa3bb36793f2e661d2f92b58c` (parent `709838df`), then corrected
      the required Rustfmt-only test layout in `ad36bc3a2ef53840ce2043ffc40cfde0145cc077`:
      `run_attachment_upload` now enforces workdir containment before reading the file,
      with regression coverage for an outside absolute path. New candidate is
      `46b5390d171bad90aad5b8ec34461d164b2c6077`; exact-head review requested once at
      comment `5395709568`; subsequent compile-fix heads superseded this candidate.
      Previous review/gate is stale.
- [x] #113 local exact-head self-review of `ad36bc3a` completed: task attachment path
      is checked before `fs::read`, outside absolute and traversal paths are rejected,
      and no new P0/P1 was found. GitHub review remains advisory/delayed; the serial
      gate may proceed under the current-thread-only execution rule.
- [ ] #113 first serial gate attempt found a changed-code compile blocker in the new
      CLI property test (`definition` borrowed `property_id` into a `'static` route).
      Minimal fix `ec0d3620de2764b97851ce793da3460fe864f0c8` (parent `ad36bc3a`) adds
      `move`; rustfmt/diff-check pass and it is pushed. Current candidate is
      `ca25863970d6f2e2a8620ed071f56e15d1ef28fd`; prior gate is invalid and must rerun.
- [ ] Rerunning #113 check found three additional changed-code agent compile blockers:
      ACP notification callback mutability, Qoder effort-choice type inference, and
      Kimi reader reborrow. Minimal fix `0a071cba399515977fd0b70e4f9fe844c6a458fb`
      (parent `ec0d3620`) passes scoped rustfmt/diff-check and is pushed. Current
      candidate is `c419ab0bb316938877e2e89af0ac9b210c7b7cda`; gate must rerun again.
- [x] Third #113 check attempt found the remaining ACP callback mutability error in
      `request_with_permission`; corrected in `0415dd6a2fd62ae6521b7a5fe2a132e7a8ddefbf`
      (parent `0a071cba`), with scoped rustfmt/diff-check pass and FF push. Current
      candidate is `89e60eb3ed057952c0c024353f3b5d535ea7e478`; gate must rerun.
- [x] #113 serial gate completed on exact head `0415dd6a2fd62ae6521b7a5fe2a132e7a8ddefbf`
      (tree and parent verified before execution). Workspace check for
      `cordy-agent`, `cordy-cli`, `cordy-daemon`, `cordy-handler`, and `cordy-server`
      all-targets passed; `cordy-server` binary check passed. Agent tests were
      `83 passed / 26 failed / 0 ignored`, and CLI tests were `97 passed / 83 failed /
      0 ignored`; every failure reached the existing sandbox process/socket
      `PermissionDenied (EPERM)` setup path before business assertions, including the
      attachment regression test, with no changed-code compile or deterministic
      assertion failure. Strict Clippy reproduced only the five pre-existing
      `heartbeat_scheduler`/`runtime_sweeper` style lints; no changed-code lint.
      The shared lock was released after confirming no cargo/rustc process and the
      restack worktree is clean. External GitHub review is still delayed; local review
      is clean, so one serial subagent read-only review is the next required decision
      point before merge.
- [x] #113 internal subagent exact-head review completed serially: PASS, no P0/P1.
      It verified canonical workdir containment before attachment `fs::read`, the
      outside-path regression, ACP callback mutability, Kimi reader reborrow, and
      Qoder type inference as behavior-preserving; no files, Cargo, or remote refs
      were changed by the subagent. External GitHub review remains advisory/delayed.
- [x] #113 merged with GraphQL expected head `0415dd6a2fd62ae6521b7a5fe2a132e7a8ddefbf`.
      Merge `4576ca30cfe9cedeb0ad4ca2ad4edaf90424da59`, parents
      `2b7d31e5f20d08c90f22c99c19ff6a0b04637a39 + 0415dd6a2fd62ae6521b7a5fe2a132e7a8ddefbf`,
      tree `5a5b238af7828bddb3b2828aae7fc221978d4067`; remote
      `codex/cord-20-daemon-control` ref verified at the merge.
- [ ] #113 original worktree
      `.worktrees/cord-22-daemon-production-assembly` has unrelated uncommitted CLI
      edits and must not be reset or used for the gate. Restacked tree has passed
      `git diff --check` and changed-Rust `rustfmt --check`.
- [x] #114 restacked non-rewriting after #113 merge. Final branch head
      `bcad0a6f335d74dea03c67ce446bfbdc6745e367`, base
      `0415dd6a2fd62ae6521b7a5fe2a132e7a8ddefbf`, tree
      `b6e95dea2ee63840e5dc0f6b75e112c9ec48f4ce`, candidate
      `617d8b9d552a85f4b5bb0b10f49df90aafe6df26`; PR was Ready/CLEAN/MERGEABLE.
      Serial internal subagent review passed with no P0/P1. Changed-code fixes
      were delegated serially and preserved as non-rewriting commits:
      `0b335a66`, `87566011`, `9b9f7f6d`, `03463099`, `3f59666d`, `bcad0a6f`.
- [x] #114 exact-head gate passed on `bcad0a6f`: daemon/server all-target checks
      passed; daemon tests `409 passed / 10 failed / 0 ignored`, with all ten
      failures reproducing pre-existing sandbox/filesystem or socket EPERM
      conditions; server tests `15 passed / 1 failed / 0 ignored`, with the sole
      failure being the unchanged black-hole Redis bind blocked by sandbox EPERM.
      Strict Clippy for `cordy-daemon` and `cordy-server` all targets passed with
      `-D warnings`; no changed-code lint. Lock was released after confirming no
      Cargo/rustc process and the worktree was clean.
- [x] #114 merged with GraphQL `expectedHeadOid=bcad0a6f335d74dea03c67ce446bfbdc6745e367`.
      Merge `7ac42ed089895a88b165ca1e83734d37429e9286`, parents
      `0415dd6a2fd62ae6521b7a5fe2a132e7a8ddefbf + bcad0a6f335d74dea03c67ce446bfbdc6745e367`,
      tree `b6e95dea2ee63840e5dc0f6b75e112c9ec48f4ce`; remote
      `codex/cord-22-daemon-production-assembly` ref verified at the merge.
- [ ] #126 → #127 → #128 → #129: process strictly in that order after daemon base moves.
- [ ] #126 is now the active item after a required non-rewriting base sync: PR head
      `bf54b309d7459beb2bb1621986e817ffe0c55764` (parents
      `30b90607e29f13e5c4610891fae36ec3edcc2251 + bcad0a6f335d74dea03c67ce446bfbdc6745e367`),
      base ref `codex/cord-23-daemon-runtime-registration` at `bcad0a6f335d74dea03c67ce446bfbdc6745e367`,
      head tree `80fa6617c42e989d710c4c689d5b336e2d9b2c1c`, and current candidate
      `beae47759f71c35c31690f9766c6661a23ff1f91`. PR is Ready/CLEAN/MERGEABLE;
      serial subagent exact-head behavior audit is in progress before any gate.

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
- [x] Refactor slice: implement the Go `login --token` contract in the
      existing Rust CLI branch `codex/cord-18-rust-cli`; a serial subagent owns
      the bounded code change while this thread owns the contract scope and
      TODO. PR review/gate/merge is delegated to the pro model.
- [x] CLI `login --token` slice committed/pushed as `23b987f9`, followed by
      `37a8af3a` on PR #91. It
      validates `mul_`/`mcn_`, enforces the human-local guard, resolves the
      server URL from flag/env/profile/default cloud, verifies `/api/me`, and
      atomically saves server URL plus clears stale workspace ID and stores the
      token under one profile lock. Success, invalid-prefix, and daemon-task
      guard tests were added; scoped rustfmt check and diff-check pass.
      Cargo/review/gate/merge remain with the pro model and are not duplicated
      here.
- [x] Daemon foreground/adapter dependency boundary slice completed on
      `codex/cord-50-daemon-command-assembly` (PR #129). Commits
      `637712cf` (`feat(cli): assemble foreground daemon inputs`),
      `3dcd0a7e` (`refactor(daemon): pass shared runtime owners to adapter`),
      and `58708109` (`refactor(daemon): derive backend config from launch
      state`), followed by `7cd69522` (`test(daemon): stabilize backend config
      assertion`) and `f1f12ab5` (`feat(daemon): build typed task execution
      plans`) are pushed; latest head
      `f1f12ab5c7cf3da6cccbc861b58dd101e6dd0c65`, parent
      `7cd695223c638cc3a8185bff8a81e889a8d157ab`, tree
      `0d482e3c85fe3682adfbe86ee4794ebfad918a7a`. PR #129 is Ready/CLEAN/
      MERGEABLE on base `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`, current
      candidate `520c9eef89ca4d275291200cb7a3ce1f34c98c8e`. The CLI now carries
      one authenticated profile snapshot through lifecycle/bootstrap/foreground
      production input assembly. `ProviderRuntimeContext` injects the same
      client, accepted `RuntimeLaunchRegistry`, activity, repo state, and
      checkout registry into the adapter boundary; `backend_config` derives
      `cordy-agent::BackendConfig` only from accepted workspace launch state and
      caller-supplied task environment, failing closed when no launch is
      registered. The new execution-plan builder maps claim data into typed
      `PrepareParams`/`TaskContextForEnv`, then binds prepared environments to
      `ExecOptions` and a child-only environment; task credentials require
      `mat_` and never fall back to the daemon token. Sensitive Debug output is
      redacted. Scoped rustfmt/diff-check pass; Cargo/review/gate/merge remain
      delegated to the pro model.
- [x] Real provider execution adapter implemented on top of
      `ProviderExecutionPlan`; the slice was first pushed as `f7e72057` and
      the current PR head is `a06019544c9cd12254c9f95c39a3690e96f3ef36`
      (parent `f7e72057dcacce6bd1a8580d4a5850830269d136`, tree
      `349d130b503d725b259c90f0fe774f216d48ac7b`). The adapter owns ordered
      prepare/reuse, local-path locking, worktree finalization, accepted launch
      resolution, real `cordy-agent` backend execution, bounded/redacted
      transcript delivery, session pinning, usage/result mapping, cancellation,
      prepare-lease extension, checkout ownership, and fail-visible unsupported
      heartbeat actions. The latest PR state is Ready (`draft=false`),
      CLEAN/MERGEABLE on base `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`, with
      candidate `da64a3d1a73095032569430e47c48f260d38dcb8` (same tree as the
      head). Scoped rustfmt for changed files and staged diff-check pass; the
      repository's existing `auto_update.rs` formatting difference still makes
      a module-wide rustfmt check noisy. Cargo/review/gate/merge remain with the
      pro model.
- [x] Added the typed production factory in the same PR: `DaemonProductionInputs`
      now keeps one `Arc<Config>` and `into_production_assembly<C>()` constructs
      the concrete adapter plus `ProviderRegistrationSource<C>` while sharing
      the authenticated client, accepted launch registry, and checkout registry.
      `DaemonStartAssembly::production_assembly` exposes that boundary to the
      CLI without introducing a no-op provider fallback. The latest PR
      candidate is `3f35ccf027217b360b06bef71bde5ab09e0420a3` (parents
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c` +
      `a06019544c9cd12254c9f95c39a3690e96f3ef36`, same tree as head).
- [x] Added `LocalProviderCatalog` and the CLI-facing
      `production_assembly_with_local_catalog` factory. It reuses daemon
      discovery and the `cordy-agent` capability/version registries, probes
      real `--version` commands with bounded process-tree cancellation,
      withholds unsupported built-ins, and reports custom-profile probe or
      minimum-version failures instead of registering an empty/unknown
      launch. Current PR #129 head is
      `3017241ddc732d75314bf2309ede3e7e05f07305` (parent
      `a06019544c9cd12254c9f95c39a3690e96f3ef36`, tree
      `ec7a02c000a76358eac9f2ab7cc10a21f27c5681`), candidate
      `e208fe4105b8520b861912c3e589bb9ba7b55858`, Ready/mergeable. Scoped
      rustfmt and diff-check pass; Cargo/review/gate/merge remain with pro.
- [x] Wired a real `cordy daemon start --foreground` command into PR #129.
      It parses successor launch flags/durations, rejects background start
      rather than pretending it works, loads the authenticated profile before
      bootstrap, reuses `run_production_daemon` for PID/log/shutdown ownership,
      and supplies one shared `RepoCheckoutRegistry` to the local catalog,
      adapter, and production stack. Current head is
      `f4ebf0606d689f357a9b10d9dbe18a99cd0c1b99` (parent
      `3017241ddc732d75314bf2309ede3e7e05f07305`, tree
      `bea59d319f3979b9ba366f7ae4f1c3f142d7dd96`), candidate
      `2609fe94f6e4fa8049dc44d972bab6fdc7ab663d`; PR remains Ready and
      MERGEABLE. Scoped rustfmt/diff-check pass; Cargo/review/gate/merge remain
      with pro.
- [x] Completed background lifecycle parity on the same assembly route:
      `cordy daemon start` now uses the real `DaemonLifecycle`, while
      `daemon restart` and `daemon stop` reuse its health identity checks,
      bounded readiness, graceful/forced shutdown, and PID-lock handoff.
      Stop is local-control-only and does not require a server token; start and
      restart retain authenticated preflight. Incomplete stop/readiness is
      reported as failure rather than a false success. Current PR #129 head is
      `e72c28b51c9139ab19ac4bc6da2e3a19387a8706` (parent
      `f4ebf0606d689f357a9b10d9dbe18a99cd0c1b99`, tree
      `dd8c84524801e7aea82beed6e82f94ceccb0fdea`), candidate
      `574d9101f5bc5715a469feb1c7b6bf918df881fb`; PR remains Ready and
      CLEAN/MERGEABLE. Scoped rustfmt/diff-check pass; Cargo/review/gate/merge
      remain with pro.
- [x] Production shutdown/successor audit completed on the frozen head
      `e72c28b5`: PID-lock ownership, health disappearance/identity checks,
      root cancellation and task drain, auto-update/reload successor argv,
      profile health-port derivation, and restart preflight all remain
      closed-loop. No new P0/P1, changed-code compile blocker, or data/lifecycle
      safety issue was found. Deferred parity differences are recorded rather
      than patched: restart does not yet accept the full start flag set; start
      timeout returns an error instead of Go's warning; and stop still loads
      profile config/server URL instead of being purely local-control-only for
      corrupt profiles. None is an urgent safety or correctness blocker.
- [x] The restart flag contract is now shared with start in PR #129 via
      `DaemonLaunchArgs::to_launch_flags`; identity, workspace root, timing,
      concurrency, health-port, auto-update/reload, and global server/profile
      precedence all flow through one resolver. `restart --foreground` is
      rejected explicitly. Current head is
      `a7827a4cb39a9a09ca8f6ad88da0b636427ffe32` (parent
      `e72c28b51c9139ab19ac4bc6da2e3a19387a8706`, tree
      `656f7d025353c2af6d8186d5fee25017e0a51509`), candidate
      `a06dacdf71ff3dd95468ceab6d5f2bbf3b65e248`; scoped rustfmt/diff-check
      pass, Cargo/review/gate/merge remain with pro.
- [ ] Next substantive slice: move to the next Go→Rust business domain; keep
      the daemon lifecycle/provider assembly single-sourced and do not reopen
      the recorded non-blocking P2 differences without new evidence.
- [x] Began the `cordy setup` domain with a typed, fail-closed profile
      replacement boundary in PR #129. `SetupProfileInput` only carries
      server/app URLs; health preflight must succeed before mutation; success
      atomically replaces the whole profile (clearing stale token/workspace and
      unknown fields) while preserving lock/permission/fsync semantics. Current
      head is now `a5419afdd78b55e4a0886c9a93eaf57616f25fcd` (parent
      `058d7761f7fa54c0dadea4c92858777a6a0d91ad`, tree
      `2763de641477a8537d6a84d556258570deb66c04`), candidate
      `510fe43797e79695a22687d0d323234f621e64db`; the remote branch and PR
      candidate were verified at these exact SHAs. PR #129 remains
      Ready/CLEAN/MERGEABLE on base `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`.
      The slice adds the bounded unauthenticated `/health` probe (HTTP(S), no
      redirects, only 200, two-second request/outer timeout), self-host/cloud
      URL precedence, and setup command dispatch. Probe failure leaves the old
      profile untouched; an environment `CORDY_TOKEN` is persisted only after
      the successful probe. After authentication, daemon health selects a
      real background start, an idle-daemon restart, or a fail-safe
      active-task deferral; lifecycle/readiness failures are propagated. The
      candidate merge ref for the new head must be refreshed after GitHub
      catches up. Scoped rustfmt/diff-check pass; Cargo/review/gate/merge
      remain with pro.
- [ ] Next setup slice: port the real interactive login flow; until that lands,
      setup without `CORDY_TOKEN` returns a typed “interactive login
      unavailable” error after the verified profile replacement, and must not
      claim completion.
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
