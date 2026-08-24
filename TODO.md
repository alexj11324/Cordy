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
      head is now `22762d880efac094d28c1e309cbdb7f7f1376f0e` (parent
      `e817370fe8ad3f2482a7235bc6cf2d17db664543`, tree
      `2a004f84c71b0c8968f2968eb7f8325a0f0d23cb`), candidate
      `5cc28510b5414d5410d1fa9193fb660d0b91b4ae`; the remote branch and PR
      candidate were verified at these exact SHAs. PR #129 remains
      Ready/CLEAN/MERGEABLE on base `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`.
      The slice adds the bounded unauthenticated `/health` probe (HTTP(S), no
      redirects, only 200, two-second request/outer timeout), self-host/cloud
      URL precedence, and setup command dispatch. Probe failure leaves the old
      profile untouched; an environment `CORDY_TOKEN` is persisted only after
      the successful probe, and an existing profile requires explicit `y/yes`
      confirmation before the destructive whole-profile replacement. After
      authentication, daemon health selects a
      real background start, an idle-daemon restart, or a fail-safe
      active-task deferral; lifecycle/readiness failures are propagated. The
      candidate merge ref for the new head must be refreshed after GitHub
      catches up. Scoped rustfmt/diff-check pass; Cargo/review/gate/merge
      remain with pro.
- [x] Added the daemon disk-usage public facade to PR #129. Scan DTOs,
      root/pattern resolution, and single/multi-root scans are now typed public
      APIs. Parent issue statuses use an injected `ParentStatusResolver` and a
      `ClientParentStatusResolver` that keeps batch→legacy fallback and
      cancellation without exposing the internal repository context; per-root
      failures remain best-effort. The legacy client fallback now treats only
      404 as not-found and preserves other issue errors. Current exact head is
      `31ed70f6fbef53eff392c8bd861e16ff1ae25e35`, tree
      `7bf7aa1794012deb3989201cb10e0091bcf61d5f`, candidate
      `e24f805172f64d01078ca41b3a4860716d4ce30c`, parent
      `62aa0af57a3d97c242b51228fd0617c6b81049c8`; PR remains Ready and
      mergeable on base `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`. Scoped
      rustfmt/diff-check pass; Cargo/review/gate/merge remain with pro.
- [x] Completed the `cordy daemon disk-usage` CLI migration on PR #129.
      The command now supports Go's task/workspace views, `--top`, table/JSON,
      `--workspaces-root`, and `--all-profiles`; it keeps full scan totals when
      rows are truncated, scopes managed tasks to the injected absolute root,
      and performs parent-status lookup only where rendered, with generic
      stderr warnings and clean JSON stdout on lookup failure. Exact head is
      `33d5df4560abe803fd193668e91ab8104b071bff`, tree
      `96a6a16e9b4795f1630035f62e72a71be89b8be4`, candidate
      `ad820ed10935d3df3692ab603c15493e294e8e09`, parent
      `31ed70f6fbef53eff392c8bd861e16ff1ae25e35`; remote head/base and
      Ready/CLEAN mergeability were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added the typed daemon root-update facade to PR #129. `UpdateRequest`
      supports explicit or latest targets and caller download timeouts;
      direct installs resolve and validate the latest tag, while Homebrew
      treats latest-release lookup as advisory and still upgrades when it
      fails. Existing download, checksum, extraction, and atomic replacement
      code remains single-sourced, and public outcomes are path-free. Exact
      head is `c0e3d8dd227111c11618c3793410cb2ae3a207ea`, tree
      `08c91422dc6db1edd7f53cab1dd92d75e98533ed`, candidate
      `25b074290da886e9c2a4022102dbef95d30da74e`, parent
      `33d5df4560abe803fd193668e91ab8104b071bff`; PR remains Ready/CLEAN.
      Scoped rustfmt/diff-check pass; Cargo/review/gate/merge remain with pro.
- [x] Added the root `cordy update` CLI command on PR #129. It owns only
      human-local/task guard, timeout parsing, current/latest/warning/output
      decisions, and calls the daemon's typed update facade; download,
      checksum, extraction, and replacement remain daemon-owned. Output is
      stderr-only and strips URLs, paths, tokens, and authorization details.
      Exact head is `22762d880efac094d28c1e309cbdb7f7f1376f0e`, tree
      `2a004f84c71b0c8968f2968eb7f8325a0f0d23cb`, candidate
      `5cc28510b5414d5410d1fa9193fb660d0b91b4ae`, parent
      `e817370fe8ad3f2482a7235bc6cf2d17db664543`; PR remains Ready/CLEAN.
      Scoped rustfmt/diff-check pass; Cargo/review/gate/merge remain with pro.
- [x] Added `cordy daemon status` to PR #129 as the next bounded CLI slice.
      It reuses the typed daemon health/control client, preserves profile-derived
      ports and profile-identity collision checks, uses the injected daemon port
      inside managed tasks, rejects task profile overrides and unknown profiles,
      and renders Go-compatible running/starting/stopped diagnostics in table or
      JSON form. Tests cover parser modes, nested profile discovery, task port
      boundaries, collision JSON, and table diagnostics. Exact head is
      `0882488ed29b3bb84d96e8f06dfda9c54dfa75b5`, parent
      `22762d880efac094d28c1e309cbdb7f7f1376f0e`, tree
      `b8fe798ee4d548671f005f204ba5537ba7f6a407`, candidate
      `84765f8571c2d64a849a19e8992bbfe912396bc0`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready/CLEAN/MERGEABLE were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy daemon logs` to the same PR. It keeps the Go human-local
      guard and profile path semantics, supports bounded `--lines/-n` and
      `--follow/-f`, tails the newest bytes without unbounded allocation,
      follows file growth through rotation until Ctrl-C, and keeps the path
      notice on stderr while log content remains stdout. Tests cover flags,
      newline/no-newline tails, and managed-task rejection. Exact head is
      `c584029b6bae4fee5654e4cc2e4827cd3aa28fa1`, parent
      `0882488ed29b3bb84d96e8f06dfda9c54dfa75b5`, tree
      `a158fc9bf67a28c0668a2440b936e4c3198d0f6d`, candidate
      `70454008e4fafaf1700a311a67d4e52c33d1e586`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added the independent `cordy squad list` CLI slice to PR #129. It uses
      the existing authenticated/workspace-scoped API client, preserves Go's
      JSON and table columns (`ID`, `NAME`, `LEADER ID`, `MEMBERS`), keeps the
      empty-list notice on stderr, and has parser, rendering, and HTTP header
      contract tests. Exact head is
      `878852c90a09507a75273b1884cf60bd6b76d1d2`, parent
      `c584029b6bae4fee5654e4cc2e4827cd3aa28fa1`, tree
      `b8f663d32fecf520a190a414e523dc71ce34db15`, candidate
      `bd3aad00766aacd84c62d13fffe2be63d1b7d45e`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready/CLEAN/MERGEABLE were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad get <squad-id>` to PR #129. JSON returns the full
      server object; table output matches Go's ID/name/description/leader/
      created fields and optional instructions. IDs are trimmed, empty IDs
      fail closed, and path segments are encoded before the authenticated,
      workspace-scoped request. Exact head is
      `913f67754e2b17f1881e3a3baead883363e43773`, parent
      `878852c90a09507a75273b1884cf60bd6b76d1d2`, tree
      `b1e4f1a2534654f2bc0e67d33f6a41dff72f8d0e`, candidate
      `e0dfca264d775e1d63214b8a6560b44bccf45ab7`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready/CLEAN/MERGEABLE were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad create` to PR #129. It validates required name and
      leader, resolves a leader by UUID or case-insensitive name through the
      existing authenticated agent resolver, sends only the Go-compatible
      fields, and renders JSON or the created-squad table line. The slice also
      corrected `squad get`'s default output to Go's table mode. Exact head is
      `e476107565736e625a28a428f03013e4c27b85b2`, parent
      `a3cebb97cc0fbad6e8c1f8c3b8aa6098f82f511d`, tree
      `d90cd6ec90935f25dc13a384b8322fe4f6f416e7`, candidate
      `afe99baf914e7345228dfc0179d21514e1e8244a`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad update <squad-id>` to PR #129. It sends only explicitly
      selected fields (including explicit empty strings), resolves `--leader`
      through the existing authenticated agent resolver, encodes the path
      segment, rejects empty IDs/no-op updates, and preserves Go-compatible
      JSON/table output. Exact head is
      `f062de0cd2752b44a9e9bce7c7ae9cf4a7f94afa`, parent
      `e476107565736e625a28a428f03013e4c27b85b2`, tree
      `40ef8006af1eaa38dca628663eeb35796f9d4675`, candidate
      `ec0295653777a6dab7054e32755be47e1818bd31`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad delete <squad-id>` to PR #129. It uses the authenticated
      workspace-scoped DELETE endpoint with an encoded ID, rejects empty IDs,
      and matches Go's stderr table notice or JSON `{id, deleted}` response.
      Exact head is `634d2f6daebd847cf1007010ee92cf51104116a6`, parent
      `f062de0cd2752b44a9e9bce7c7ae9cf4a7f94afa`, tree
      `b522993d8bff7c02d509a134c6105887ea0eb0dc`, candidate
      `50bec69900617594d727fd8b07d97a377164f7db`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad member list <squad-id>` to PR #129. It reuses the
      authenticated client, encodes the squad path, preserves the Go table
      columns/empty-list notice, and returns the server array unchanged in
      JSON mode. Exact head is
      `1ee93aadc97732671d01d0611f6e781983473660`, parent
      `634d2f6daebd847cf1007010ee92cf51104116a6`, tree
      `a45837494af1f509b5a0292c90c0551e32bd7652`, candidate
      `963c90f0e3f0dcecfa7dfbacf93086c459ddd8ca`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad member add <squad-id>` to PR #129. It validates the
      member ID and `agent|member` type, preserves Go defaults for type/role,
      posts the workspace-scoped body, and renders JSON or the stderr table
      notice. Exact head is
      `425e687dd83adaf2cafa04e9fcfe893926b51141`, parent
      `1ee93aadc97732671d01d0611f6e781983473660`, tree
      `ab8678e121a63333326a2130387a4ebf5cfcb038`, candidate
      `948a4f4a5553baa0fa64cae47afd647f0c4b3c69`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad member set-role <squad-id>` to PR #129. It validates
      member ID/type/role, sends the Go-compatible PATCH body through the
      authenticated workspace client, and preserves JSON or stderr table
      output. Exact head is
      `8d67f4e6b0f237111b68085161aaa0092d1114c6`, parent
      `425e687dd83adaf2cafa04e9fcfe893926b51141`, tree
      `4e8a6e2e5348720f023ba2a0c1f9f72dd8e7cf9f`, candidate
      `f276b8dd79d5d30d72bf245d21ee2dba9894d13d`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad member remove <squad-id>` to PR #129. It adds the
      minimal typed DELETE-with-JSON-body API client method, validates member
      ID/type, sends the Go-compatible body, and preserves JSON/table output.
      Exact head is
      `767c3a306321544af96e88b0fa15e1f665ce04df`, parent
      `8d67f4e6b0f237111b68085161aaa0092d1114c6`, tree
      `57db24ebf880204ddf60db6c5ad142c6def54125`, candidate
      `b1f389981dfe9893d87f7ab7e2066ba2b617ad87`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy squad activity <issue-id> <outcome>` to PR #129. It validates
      the three Go outcomes, resolves the issue reference before posting the
      workspace-scoped evaluation body, and preserves stderr/table versus JSON
      output. Exact head is
      `6b4620b43b2eea88e36a244ee1e5a7dcea662441`, parent
      `767c3a306321544af96e88b0fa15e1f665ce04df`, tree
      `604eb6b2c78a34dce4592893d8b12ca8236967a2`, candidate
      `52e6cf2463603b1a3a7170f6e646824cea51909c`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill list` to PR #129. It introduces the missing top-level
      workspace skill listing, preserves the Go JSON array and table columns
      (`ID`, `NAME`, `DESCRIPTION`, `CREATED_AT`), and reuses the authenticated
      workspace client. Exact head is
      `6fbeb9effe3636af206ec654ea04f3cc2f608284`, parent
      `6b4620b43b2eea88e36a244ee1e5a7dcea662441`, tree
      `9b28107f1a7f0475c75cac24fd675bb08a56b06d`, candidate
      `e2fbf99c1b1aa25bc7d11cfbddd13628ab3d05e2`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill get <id>` to PR #129. It fetches an encoded skill ID,
      preserves the complete JSON object, and provides Go-compatible table
      output for ID/name/description/created time. Exact head is
      `16f407b4954e048e1695a44b8fd56c7d745f38e1`, parent
      `6fbeb9effe3636af206ec654ea04f3cc2f608284`, tree
      `90759c0faeaca50f526d9d57bc013fe33ec8cd21`, candidate
      `7854b53b16bc4c91eaa69f5ee5c6586ad846b480`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill create` to PR #129. It validates the required name,
      preserves mutually exclusive inline/stdin/file content sources and UTF-8
      bytes, validates JSON config, posts the workspace-scoped payload, and
      matches Go's JSON/table output. Empty explicit content follows Go and is
      omitted rather than rejected. Exact head is
      `6c1287368ef7d71d236245d3f89f57b32b9b8168`, parent
      `16f407b4954e048e1695a44b8fd56c7d745f38e1`, tree
      `1ca135462c410a1a46c6986fe477de5da14c5e39`, candidate
      `db24125e0f58fb1c9b7549cee6749c0d7a68002c`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill update <id>` to PR #129. It reuses the content-source
      helper, preserves explicitly empty name/description/content fields,
      validates JSON config, rejects no-op updates, and matches Go's PUT and
      JSON/table outputs. Exact head is
      `a027f1b35124caa6ea8ea815406adc27a88ce405`, parent
      `6c1287368ef7d71d236245d3f89f57b32b9b8168`, tree
      `c18224fb705774a131f3b73a4cb281074f07c61a`, candidate
      `7e6e754bd2f6280f5da17bf2dacde96584e0f4c7`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill delete <id>` to PR #129. It preserves Go's explicit
      confirmation/`--yes` behavior, fail-closed empty IDs, encoded DELETE
      path, and success/abort output; tests cover accept, decline, EOF, and
      authenticated request headers. Exact head is
      `a9b047ff40ccf7365847a4f740f1e5f21d4c86f4`, parent
      `a027f1b35124caa6ea8ea815406adc27a88ce405`, tree
      `eee1ad4740be2c2d3f9da54aad5f360e910d93e2`, candidate
      `cfd05feefc65c8146e9e55275ba0b9f718e15878`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill refresh <id>` to PR #129. It posts the empty JSON
      refresh body to the encoded source-backed skill endpoint, preserves JSON
      and table output, and reuses auth/workspace headers. Exact head is
      `6faf2ef64e0a3757a681d128406745478e0a14f6`, parent
      `a9b047ff40ccf7365847a4f740f1e5f21d4c86f4`, tree
      `171366c6b8534d18b5e43404f9848cbb2cfb7c0b`, candidate
      `10032e0306515e2e8327a8e60841ac0c4c751b3b`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill search <query>` to PR #129. It trims and validates the
      query, uses URL-encoded `/api/skills/search?q=...`, and matches Go's
      installable-skill JSON/table columns and headers. Exact head is
      `5121aaad75fb7dcb8150a4bf48cd0eae812078ba`, parent
      `6faf2ef64e0a3757a681d128406745478e0a14f6`, tree
      `8c0a074b84b1d20830859334e6a4a925638d6903`, candidate
      `a0f01368e6d08b02ce2dba435f19dbbc242d1708`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Closed the update facade's already-current behavior gap on PR #129:
      explicit/latest targets are normalized against an optional current
      version and return a typed no-op before download or Homebrew execution;
      Homebrew still upgrades when latest lookup fails. Exact head is
      `e817370fe8ad3f2482a7235bc6cf2d17db664543`, tree
      `0036a576c978ac4e9810dcb78014e01982d1d810`, candidate
      `5b33beb510aadf02de0422502e4da89da996b80e`, parent
      `c0e3d8dd227111c11618c3793410cb2ae3a207ea`; scoped rustfmt/diff-check
      pass, Cargo/review/gate/merge remain with pro.
- [x] Added the hidden `cordy daemon probe-runtimes` command through a typed
      daemon facade in PR #129. The facade reuses the complete daemon
      `load_config`/agent discovery path with `AllowNoAgents=true`, preserves
      profile command and OpenClaw overrides, and keeps tokens and resolved
      executable paths out of the JSON report. The slice checkpoint was
      `62aa0af57a3d97c242b51228fd0617c6b81049c8` (parent
      `bb2079c9c206a41a8fb0188690b9116d7381e275`, tree
      `67e4e95453d77408a7558a2b068d58f2223532f7`), candidate
      `cd8a715cf5b6a5a6380598dab89148e53c19b434` (parents base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c` + head), remote and PR
      `head/base/draft/mergeable` were verified exact and Ready/CLEAN. Scoped
      `git diff --check` passed; rustfmt is unavailable in this execution
      image (the slice was formatted before handoff), and Cargo/review/gate/
      merge remain with pro.
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
