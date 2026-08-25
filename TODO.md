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

## Current stack split decision (2026-08-25)

- [x] Do not extend aggregate PR #159. Its exact base was `9b38ea94151514f3b49b26b1d8e4d32d4a42016f`,
      head is `b2301f0263b49ded70404342bfb7f9a59e273d75`, and the aggregate diff is
      93 files / 18,498 additions / 15,620 deletions. This is too broad for a
      maintainable stack PR and was closed as `SUPERSEDED` (comment
      `5404653797`).
- [x] Create and push Ready, non-rewriting stack branches in this order, keeping each
      child based on the exact parent branch. PR/base/head/tree evidence:
      1. #160 `codex/cord-50-cli-foundation`: base `main@9b38ea94`, head
         `154df0b015a9a23ecd9c07720a5a9ffc6215ce93`, tree
         `a85cc0df438a194ae70c586d0087f65260d0659a` (behavior/fix foundation);
      2. #161 `codex/cord-50-cli-daemon`: base `#160@154df0b0`, head
         `03c9c43bf2dc6868010b8b52c3c7e3bb62366a1f`, tree
         `d9bb6548041db709136e140b0a1408b126813cf5` (daemon/auth/setup/runtime policy);
      3. #162 `codex/cord-50-cli-platform`: base `#161@03c9c43b`, head
         `1684d444e6d7569e821a444c901145d13414d232`, tree
         `394528df29f32343c963866db2108b736d5d434a` (autopilot/repository/chat/skill);
      4. #163 `codex/cord-50-cli-workspace`: base `#162@1684d444`, head
         `53bd13f7879eb9188c18ba69137579dadc8e3115`, tree
         `985006c482c3878910f773e285fcd61af00748a9` (property/squad/workspace);
      5. #164 `codex/cord-50-cli-agents`: base `#163@53bd13f7`, head
         `0fa943d55bcdd14746bfab1b1e14b2a79beeeb5b`, tree
         `f558cb4a90ff995f818e778cfc8d1905535b1703` (user/label/project/agent);
      6. #165 `codex/cord-50-cli-issues`: base `#164@0fa943d5`, head
         `8975b4a8c4b2210ea13041889672d0f6d7f32ca6`, tree
         `137076e92dba9fa5f2eab9748d9edbc27042808a` (issue command/refactor surface);
      7. #166 `codex/cord-50-cli-core`: base `#165@8975b4a8`, head
         `c43939b370142a0586d20a4c53418195e7a5fe70`, tree
         `43042c5e9631386e44ab47c3f54960500ca7d1e6` (shared client/output/helper policy);
      8. #167 `codex/cord-50-cli-schemas`: base `#166@c43939b3`, head
         `b2301f0263b49ded70404342bfb7f9a59e273d75`, tree
         `a6533ad745edac2465ea2e13a78c7df58e7a8253` (command schema modules).
- [x] #160→#167 are all Ready (`isDraft=false`), CLEAN, and non-rewriting. Current
      candidate merge SHAs are #160 `196a336cdee59eb5fda9c48ba25caa793cda0bfe`,
      #161 `8656edc3143e1ba25fcda568f5aab9e65fafc877`, #162
      `61337b99b78f999169eba8bda1e9b96f518c26c4`, #163
      `76cc2c9bae3f97d1eddba112d9250b6fd8461d89`, #164
      `b05a61a432f8bfda66f8dd92d5b7c77209308a82`, #165
      `257178689458e30d7c7fa2588599c658a6d4bdbc`, #166
      `796087ac64df1ee4612c84c33df8178923bf5027`, and #167
      `087babdbe2425426bcd227a343d35f615c0dbfca`. They remain serial; Pro owns
      review, Cargo gates, and merge.
- [x] Next structural slice is #168, `codex/cord-50-cli-skill-schema`, based on
      #167 at `b2301f0263b49ded70404342bfb7f9a59e273d75`; exact head
      `9941a51b75b7571846ede20534c05766156eb456`, tree
      `779c9827541c27bb0de6feae97b45fb9c0e66443`, candidate
      `54c3a3ca7e2d70b7b21120a59dd3b63b5981ade6`. It moves only Skill parser/schema types to
      `skill_command_schema.rs`; execution behavior remains in `skill_commands.rs`.
      New schema rustfmt and `git diff --check` pass; Cargo/review/gate are delegated
      and have not been run by this thread. Subagent exact-head review PASS: clap
      fields/defaults, `PathBuf` semantics, re-exports, handlers, dispatch, and tests
      have no compile/behavior blocker. PR is Ready (`isDraft=false`).
- [x] Next structural slice #169, `codex/cord-50-cli-workspace-schema`, is based on
      #168 at `9941a51b75b7571846ede20534c05766156eb456`; exact head
      `145866c38861194779e3314f5859162e40fc807b`, tree
      `100c8a5193baa8422db2e15feb513f247bb1c51f`, candidate
      `45ad499ea9cd45f1ef227dd2b7d7e15c4f4d07da`. It moves only workspace/member/MCP
      parser types to `workspace_command_schema.rs`; execution behavior is unchanged.
      New schema rustfmt and `git diff --check` pass; Cargo/review/gate are delegated
      and have not been run by this thread. PR is Ready (`isDraft=false`).
- [x] #173 exact-head subagent review PASS: public `Cli` re-export, global flags,
      all command variants, Parser metadata, `Command` visibility, dispatch/submodule
      references, tests, and main `Cli::parse()` compatibility have no blocker. Cargo
      remains delegated.
- [x] Next structural slice #173, `codex/cord-50-cli-command-registry`, is based on
      #172 at `192b888c3f4caca725c1eff26503207e96acbadf`; exact head
      `98df028c30e763b323dcb7b6875cc93fc3ee9dcc`, tree
      `1b56f12ff2d41e01d0687bba81ae165356471aa0`, candidate
      `a91c86ee61d7be3181459d3494de48b68d6cac32`. It moves only the public Cli parser
      and root Command registry to `cli_command_schema.rs`; dispatch and subcommand
      behavior are unchanged. New schema rustfmt and `git diff --check` pass; Cargo,
      review, and gate are delegated and have not been run by this thread. PR is Ready.
- [x] PR #174 `codex/cord-50-cli-root-tests` is a bounded child of #173: base
      `98df028c30e763b323dcb7b6875cc93fc3ee9dcc`, exact head
      `315a3d798a9767f9dfe98abf4781a5e4ebfed33c`, tree
      `386c3d54cfff33ce86de798f59bbcf86ba5ec9b3`, candidate
      `a1fa6d10310dab0ce359efc40406b7959fe67466`. It extracts only version/update
      contract tests into `root_command_tests.rs`; production behavior is unchanged.
      New test module rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`,
      CLEAN/MERGEABLE). Cargo and review remain delegated.
- [x] #174 exact-head subagent review PASS: `root_command_tests.rs` can access the
      parent crate's private/re-exported API through `use super::*`; all six moved
      tests and assertions are preserved, with no naming collision or P0/P1/compile
      blocker. Subagent did not run Cargo or modify the worktree; coordinator's
      rustfmt check and `git diff --check` both pass.
- [x] PR #175 `codex/cord-50-cli-daemon-tests` is the next bounded child of #174:
      base `315a3d798a9767f9dfe98abf4781a5e4ebfed33c`, exact head
      `c3e5310cc06eff79986283344e5ace16067f503d`, tree
      `828c70069d6222357fe3d5912d40d499ed5caad6`, candidate
      `b2bf6ada8c749731812854c33a578040301a2bf4`. It extracts only daemon auth,
      foreground/start, restart, probe, status, and logs contract tests into
      `daemon_command_tests.rs`; production behavior is unchanged. New module
      rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`,
      MERGEABLE/UNSTABLE). Cargo and review remain delegated.
- [x] #175 exact-head subagent review PASS: `daemon_command_tests.rs` accesses the
      parent crate through `use super::*`; all 13 daemon auth/start/restart/probe/
      status/logs tests and assertions are preserved, with no P0/P1 or compile
      blocker. Disk-usage tests intentionally remain in the original module for a
      later bounded slice. Subagent did not run Cargo or modify the worktree;
      coordinator's rustfmt and `git diff --check` pass.
- [x] PR #176 `codex/cord-50-cli-disk-usage-tests` is the next bounded child of #175:
      base `c3e5310cc06eff79986283344e5ace16067f503d`, exact head
      `57f2dc0e03d7972df7ea9c27a4ab9148f986a4d8`, tree
      `729bc7e24120765b42fedbd9479ee07a27bd27b5`, candidate
      `0c7c641693d4249ed659225663df68d32c4996d3`. It extracts only disk-usage
      parser/validation/limiting/formatting/profile-root tests into
      `disk_usage_command_tests.rs`; production behavior is unchanged. New module
      rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`,
      MERGEABLE/UNSTABLE). Cargo and review remain delegated.
- [x] #176 exact-head subagent review PASS: `disk_usage_command_tests.rs` accesses
      the parent crate through `use super::*`; all 8 disk-usage parser/validation/
      limit/format/profile-root tests and assertions are preserved, with no P0/P1
      or compile blocker. Subagent did not run Cargo or modify the worktree;
      coordinator's rustfmt and `git diff --check` pass.
- [x] PR #177 `codex/cord-50-cli-setup-tests` is the next bounded child of #176:
      base `57f2dc0e03d7972df7ea9c27a4ab9148f986a4d8`, original head
      `3bfefc9bc52463633c8ac20097666769c1b1fc28` was superseded by the minimal
      import fix; current exact head is
      `2aaebaa6c8b0218814d6e5f0e50ec0301cb32b6f`, tree
      `757875ad76ac3e17c694f482948270f9605685bf`. The import fix is
      `3c25f0fe8f80736bc367383ad7d0ff64a47228fd3`; a separate rustfmt import-order
      commit `2aaebaa6` is now the final head. It extracts only setup
      cloud/self-host parser, preflight, profile replacement, confirmation, daemon
      handoff, and URL-normalization tests into `setup_command_tests.rs`; production
      behavior is unchanged. New module rustfmt and `git diff --check` pass; PR is
      Ready (`isDraft=false`); candidate must be re-read after GitHub refresh. Cargo
      and review remain delegated; current candidate is `ec928eda3fbae82b9b935c8c02f2e69211e3d340`; any
      review/gate for `3bfefc9b` is invalid.
- [x] #177 final exact-head subagent review PASS on `2aaebaa6c8b0218814d6e5f0e50ec0301cb32b6f`:
      the follow-up only orders `axum` imports; `StatusCode`, `get`, `Router`,
      `Arc`, `Mutex`, and `TcpListener` plus all prior dependency fixes remain
      present. No compile/behavior blocker was introduced. Subagent did not run
      Cargo or modify the worktree; coordinator rustfmt and `git diff --check` pass.
- [x] PR #178 `codex/cord-50-cli-login-tests` is the next bounded child of #177:
      base `2aaebaa6c8b0218814d6e5f0e50ec0301cb32b6f`, exact head
      `8fdef9ba210096fda1915c4372a4958353025700`, tree
      `0ee68bfb48f765cdabe489e3f7e9e2f7dbadde17`, candidate
      `033a9d4b57f12294be38d19f8128bbc77ed446be`. It extracts only login/browser
      callback state, safe URL, authenticated profile reset, and workspace-creation
      polling tests into `login_command_tests.rs`; production behavior is unchanged.
      New module rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`,
      CLEAN/MERGEABLE). Cargo and review remain delegated.
- [x] #178 exact-head subagent review PASS: `login_command_tests.rs` accesses the
      parent crate's private/re-exported API; all 6 login/browser/profile/workspace-
      creation tests and assertions are preserved, external imports cover actual
      references, and no P0/P1 or compile blocker exists. Subagent did not run Cargo
      or modify the worktree; coordinator rustfmt and `git diff --check` pass.
- [x] PR #179 `codex/cord-50-cli-runtime-tests` is the next bounded child of #178:
      base `8fdef9ba210096fda1915c4372a4958353025700`, original head
      `592c0f1664b5bc64429ca0f8827c49727ccedefd` was superseded by a minimal import
      fix; current exact head is `051dd0bea993ac7343d3e79eba15cfcde8cd9274`, tree
      `b654d2bf6774d2ac9c266f8e74b368cccc09eeb4`, candidate
      `5e5692f4eafae0a698d98ae5119d6969b864bc91`. It extracts only runtime
      list/usage/activity/rename/delete/update and runtime-profile lifecycle/path
      tests into `runtime_command_tests.rs`; production behavior is unchanged.
      New module rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`,
      CLEAN/MERGEABLE). Cargo and review remain delegated; any review/gate for
      `592c0f16` is invalid.
- [x] #179 final exact-head subagent review PASS on `051dd0bea993ac7343d3e79eba15cfcde8cd9274`:
      `Cursor` covers all 19 `Cursor::` references, and all 11 runtime/runtime-profile
      tests remain intact. No compile or behavior blocker was introduced. Subagent
      did not run Cargo or modify the worktree; coordinator rustfmt and
      `git diff --check` pass.
- [x] PR #180 `codex/cord-50-cli-agent-tests` is the next bounded child of #179:
      base branch `codex/cord-50-cli-runtime-tests` at `051dd0bea993ac7343d3e79eba15cfcde8cd9274`,
      exact head `88b516582fd316e9741005e2a3d848eb636909dc`, tree
      `c3b8255b562197d527fadf20ae344430588b833b`, candidate
      `abe611cee2363ace2e8d79aab131606acbea6be5`. It extracts the 19 agent
      list/get/create/update/lifecycle/avatar/skills/env/MCP/copy contract tests
      into `agent_command_tests.rs`; production behavior is unchanged. Scoped
      rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`),
      CLEAN/MERGEABLE. Cargo and exact-head review remain delegated.
- [x] #180 exact-head subagent review PASS on `88b516582fd316e9741005e2a3d848eb636909dc`:
      `agent_command_tests.rs` preserves all 19 `agent_*` tests removed from
      `lib.rs`; imports, `super::*` visibility, and module boundaries are valid.
      No missing/duplicated tests or compile/behavior blocker was found. Subagent
      did not run Cargo or modify the worktree; coordinator scoped rustfmt and
      `git diff --check` pass.
- [x] PR #181 `codex/cord-50-cli-skill-tests` is the next bounded child of #180:
      base branch `codex/cord-50-cli-agent-tests` at
      `88b516582fd316e9741005e2a3d848eb636909dc`, exact head
      `13209c98fcc801e77b1ffeaa9e849761b0152a50`, tree
      `c82220b8eaf8709c14ace5d26634b08d46498fb3`, candidate
      `9949a5d5ac35ded5418f6d327c86a63311c7173e`. It extracts the 14 skill
      list/get/create/update/delete/import/refresh/search/files contract tests
      into `skill_command_tests.rs`; production behavior is unchanged. Scoped
      rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`),
      CLEAN/MERGEABLE. Cargo and exact-head review remain delegated.
- [x] #181 exact-head subagent review PASS on `13209c98fcc801e77b1ffeaa9e849761b0152a50`:
      `skill_command_tests.rs` preserves all 14 `skill_*` tests removed from
      `lib.rs`; imports, parent-private API visibility, and module boundaries are
      valid. No missing/duplicated tests or compile/behavior blocker was found.
      Subagent did not run Cargo or modify the worktree; coordinator scoped
      rustfmt and `git diff --check` pass.
- [x] PR #182 `codex/cord-50-cli-autopilot-tests` is the next bounded child of #181:
      base branch `codex/cord-50-cli-skill-tests` at
      `13209c98fcc801e77b1ffeaa9e849761b0152a50`, exact head
      `70c07b6f7539d7259a1ccfdf415010d4b28917f6`, tree
      `ca8ea6ecffd94d79373fe79ab4addd09b05b22b5`, candidate
      `6eb00da097d255c0a264a35e26f441bfd0e4911a`. It extracts the 15 autopilot
      list/get/create/update/delete/trigger/runs/resolver contract tests into
      `autopilot_command_tests.rs`; production behavior is unchanged. Scoped
      rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`),
      MERGEABLE (CodeRabbit pending). Cargo and exact-head review remain delegated.
- [x] #182 exact-head subagent review PASS on `70c07b6f7539d7259a1ccfdf415010d4b28917f6`:
      `autopilot_command_tests.rs` preserves all 15 `autopilot_*` tests removed
      from `lib.rs`; imports, parent-private API visibility, and module boundaries
      are valid. No missing/duplicated tests or compile/behavior blocker was found.
      Subagent did not run Cargo or modify the worktree; coordinator scoped
      rustfmt and `git diff --check` pass.
- [x] PR #183 `codex/cord-50-cli-workspace-tests` is the next bounded child of #182:
      base branch `codex/cord-50-cli-autopilot-tests` at
      `70c07b6f7539d7259a1ccfdf415010d4b28917f6`, exact head
      `23721319518ce8b0c170b3626718f136277237f1`, tree
      `79d149b87515d0730a563f663a332565d13824af`, candidate
      `5f80e6936587dccf23095a8b9a48a102ddef5a5b`. It extracts the 18 workspace,
      member, and MCP list/get/create/update/switch contract tests into
      `workspace_command_tests.rs`; production behavior is unchanged. Scoped
      rustfmt and `git diff --check` pass; PR is Ready (`isDraft=false`),
      CLEAN/MERGEABLE. Cargo and exact-head review remain delegated.
- [x] #183 exact-head subagent review PASS on `23721319518ce8b0c170b3626718f136277237f1`:
      `workspace_command_tests.rs` preserves all 18 workspace/member/MCP tests
      removed from `lib.rs`; imports, local `AtomicUsize` scope, and module
      boundaries are valid. No missing/duplicated tests or compile/behavior
      blocker was found. Subagent did not run Cargo or modify the worktree;
      coordinator scoped rustfmt and `git diff --check` pass.
- [x] PR #184 `codex/cord-50-cli-squad-tests` is the next bounded child of #183:
      base branch `codex/cord-50-cli-workspace-tests` at
      `23721319518ce8b0c170b3626718f136277237f1`, exact head
      `e12d80a1213155d40037a87aedf73616c3730f12`, tree
      `8952863ac9f179492178a6e53cd09d8a84c2e32f`, candidate
      `8f7fa6bf9457f38e2afb5f2401a5c69382152752`. It extracts the 12 squad,
      member, and activity contract tests into `squad_command_tests.rs`;
      production behavior is unchanged. Scoped rustfmt and `git diff --check`
      pass; PR is Ready (`isDraft=false`), CLEAN/MERGEABLE. Cargo and exact-head
      review remain delegated.
- [x] #184 exact-head subagent review PASS on
      `e12d80a1213155d40037a87aedf73616c3730f12`: `squad_command_tests.rs`
      preserves all 12 squad/member/activity tests; imports and parent-private
      helper visibility cover actual usage, with no missing, duplicated, compile,
      or behavior blocker. Subagent did not run Cargo or modify the worktree;
      coordinator `git diff --check` remains clean.
- [x] PR #185 `codex/cord-50-cli-property-tests` is the next bounded child of #184:
      base branch `codex/cord-50-cli-squad-tests` at
      `e12d80a1213155d40037a87aedf73616c3730f12`, exact head
      `3e961b11741b006e20186e1ec48dee56b38e516d`, tree
      `e78837f0f1652586c592643097accc39b78d87f5`, candidate
      `20ef4d9e08f8ffde6a18219d919ee5c2d2a7b60c`. It extracts seven property
      and issue-property parser, archive, typed-value, actor-display, and output
      contract tests into `property_command_tests.rs`; production behavior is
      unchanged. The exact-head static review found and fixed a missing
      `TcpListener` import in `3e961b11`; scoped diff-check passes and PR #185 is Ready
      (`isDraft=false`), CLEAN/MERGEABLE. Cargo and exact-head review remain
      delegated; the pre-fix head and candidate are invalid.
- [x] #185 final exact-head subagent review PASS on
      `3e961b11741b006e20186e1ec48dee56b38e516d`: `tokio::net::TcpListener`
      import covers all four uses, all seven property/issue-property tests match
      the pre-extraction baseline, and module visibility is valid. No additional
      compile or behavior blocker was found. Subagent did not run Cargo or modify
      the worktree; coordinator `git diff --check` remains clean.
- [x] PR #186 `codex/cord-50-cli-issue-search-tests` is the next bounded child of
      #185: base branch `codex/cord-50-cli-property-tests` at
      `3e961b11741b006e20186e1ec48dee56b38e516d`, exact head
      `7efffb03c1887042b4ae753f1331c7361f38a0f8`, tree
      `ea5397994ce3e9ff6e0db84b2cf7d47d2eef2ff3`, candidate
      `d7244db6da15881e28b79d4b010ed9e8e53450e9`. It extracts the two issue-search
      parser/table and HTTP contract tests into `issue_search_command_tests.rs`;
      query encoding, flags, JSON envelope, and table rendering are unchanged.
      The exact-head static review found and fixed a missing `TcpListener` import
      in `7efffb03`; scoped diff-check passes and PR #186 is Ready
      (`isDraft=false`), CLEAN/MERGEABLE. Cargo and exact-head review remain
      delegated; the pre-fix head and candidate are invalid.
- [x] #186 final exact-head subagent review PASS on
      `7efffb03c1887042b4ae753f1331c7361f38a0f8`: `TcpListener` import is
      present, both issue-search tests are intact, and parent helper/formatter
      visibility plus module boundaries are valid. No additional compile or
      behavior blocker was found. Subagent did not run Cargo or modify the
      worktree; coordinator `git diff --check` remains clean.
- [x] PR #187 `codex/cord-50-cli-issue-subscriber-tests` is the next bounded child
      of #186: base branch `codex/cord-50-cli-issue-search-tests` at
      `7efffb03c1887042b4ae753f1331c7361f38a0f8`, exact head
      `747b97c133a5e412dfdace0989a5f3296f2270f3`, tree
      `307bdf21293d7b684e493032a9d5a3907992e77d`, candidate
      `2b759f56ad5972db11ce5814ce4a287609ddced9`. It extracts the three issue-
      subscriber parser, list, and mutation contract tests into
      `issue_subscriber_command_tests.rs`; caller defaults, member resolution,
      payloads, and table/JSON output are unchanged. Scoped rustfmt and
      `git diff --check` pass. The exact-head static review found and fixed
      missing `HashMap`, `Arc`, and `Mutex` imports in `747b97c1`; PR #187 is Ready
      (`isDraft=false`),
      CLEAN/MERGEABLE. Cargo and exact-head review remain delegated.
- [x] #187 final exact-head subagent review PASS on
      `747b97c133a5e412dfdace0989a5f3296f2270f3`: `HashMap`, `Arc`, and `Mutex`
      imports are present, all three subscriber tests remain intact, and module
      boundary/visibility is valid. No additional compile or behavior blocker
      was found. Subagent did not run Cargo or modify the worktree; coordinator
      `git diff --check` remains clean.
- [x] PR #188 `codex/cord-50-cli-issue-label-tests` is the next bounded child of
      #187: base branch `codex/cord-50-cli-issue-subscriber-tests` at
      `747b97c133a5e412dfdace0989a5f3296f2270f3`, exact head
      `66e27b87f6304a57011fa2c28f836edc5f5d5fe7`, tree
      `0de99dc8d49a4161d2e734a40abb6cfc383659a5`, candidate
      `d7bf40000f159f99154f909e0f6ea67aa36b0897`. It extracts the three issue-
      label parser, add, and remove contract tests into `issue_label_command_tests.rs`;
      prefix resolution, delete-refresh fail-soft behavior, and table/JSON output
      are unchanged. Scoped rustfmt and `git diff --check` pass; PR #188 is Ready
      (`isDraft=false`), MERGEABLE (checks settling). Cargo and exact-head review
      remain delegated.
- [x] #188 exact-head subagent review PASS on
      `66e27b87f6304a57011fa2c28f836edc5f5d5fe7`: all three issue-label tests
      are preserved, delete/get routes and imports are correct, and parent
      formatter/handler visibility is valid. No compile or behavior blocker was
      found. Subagent did not run Cargo or modify the worktree; coordinator
      `git diff --check` remains clean.
- [x] PR #189 `codex/cord-50-cli-issue-metadata-tests` is the next bounded child
      of #188: base branch `codex/cord-50-cli-issue-label-tests` at
      `66e27b87f6304a57011fa2c28f836edc5f5d5fe7`, exact head
      `cbefb24e82f8b06c500cdb91973863712637b251`, tree
      `1fd18feaad2d2f7e7ce836347d4f64ec2e9663b5`, candidate
      `6f8c1df273cc1a3732eb2a49500c4bc11b491d2d`. It extracts the three issue-
      metadata parser, value-coercion, not-found fallback, and typed-update
      contract tests into `issue_metadata_command_tests.rs`; JSON/table output
      and request payloads are unchanged. Scoped rustfmt and `git diff --check`
      pass; PR #189 is Ready (`isDraft=false`), CLEAN/MERGEABLE. Cargo and
      exact-head review remain delegated.
- [x] #189 exact-head subagent review PASS on
      `cbefb24e82f8b06c500cdb91973863712637b251`: all three metadata tests are
      preserved, imports and get/put routes are correct, and typed payload plus
      not-found fallback behavior remain intact. No compile or behavior blocker
      was found. Subagent did not run Cargo or modify the worktree; coordinator
      `git diff --check` remains clean.
- [x] PR #190 `codex/cord-50-cli-issue-timeline-tests` is the next bounded child
      of #189: base branch `codex/cord-50-cli-issue-metadata-tests` at
      `cbefb24e82f8b06c500cdb91973863712637b251`, exact head
      `5b40696eaaa50e72827e4bd45c52bfb8f9a27542`, tree
      `d160a276a3412122e628d93e6b3bbd04584a7a4a`, candidate
      `497c0d434de218d309381888d501ab91f4ef4b6c`. It extracts the three issue-
      timeline parser, filtering, validation, truncation, and rendering tests
      into `issue_timeline_command_tests.rs`; history alias, RFC3339/tail
      validation, activity filtering, and notices are unchanged. The exact-head
      static review found and fixed a missing `HashMap` import in `5b40696e`;
      scoped diff-check passes and PR #190 is Ready (`isDraft=false`),
      CLEAN/MERGEABLE. Cargo and exact-head review remain delegated.
- [x] #190 final exact-head subagent review PASS on
      `5b40696eaaa50e72827e4bd45c52bfb8f9a27542`: `HashMap`, `HeaderMap`, and
      `TcpListener` imports are present, all three timeline tests remain intact,
      and module boundary/helper visibility is valid. No compile or behavior
      blocker was found. Subagent did not run Cargo or modify the worktree;
      coordinator `git diff --check` remains clean.
- [x] PR #191 `codex/cord-50-cli-chat-tests` is the next bounded child of #190:
      base branch `codex/cord-50-cli-issue-timeline-tests` at
      `5b40696eaaa50e72827e4bd45c52bfb8f9a27542`, exact head
      `30cdf04a076558ceb3e41032dca88d922d75bce8`, tree
      `e3b4e7d8d3f7358dc4847ad9fab66205638bc6d0`, candidate
      `ecfa22c8a515456cfd6d1a01430a4c67e1e847a3`. It extracts the chat
      history/thread query and rendering contract test into `chat_command_tests.rs`;
      cursor/limit encoding, thread lookup, table output, and unavailable-thread
      handling are unchanged. Scoped rustfmt and `git diff --check` pass; PR #191
      is Ready (`isDraft=false`), CLEAN/MERGEABLE. Cargo and exact-head review
      remain delegated.
- [x] #191 exact-head subagent review PASS on
      `30cdf04a076558ceb3e41032dca88d922d75bce8`: chat history/thread test,
      routes, encoded query behavior, imports, and module visibility are intact.
      No compile or behavior blocker was found. Subagent did not run Cargo or
      modify the worktree; coordinator `git diff --check` remains clean.
- [x] PR #192 `codex/cord-50-cli-repo-tests` is the next bounded child of #191:
      base branch `codex/cord-50-cli-chat-tests` at
      `30cdf04a076558ceb3e41032dca88d922d75bce8`, exact head
      `84746f9e23a277411905907c345bf7bede0e1841`, tree
      `2f6cc4b7326d4dfba6cbc3065afe318a35746e2a`, candidate
      `8351edf652e7214da353d14a1e51b3960e797fa4`. It extracts four repo
      registry/checkout parser, validation, patch, task-context, retry, and
      Retry-After contract tests into `repo_command_tests.rs`; production behavior
      is unchanged. The exact-head static review found and fixed missing
      `Arc`/`Mutex` imports in `84746f9e`; scoped diff-check passes and PR #192 is Ready
      (`isDraft=false`), CLEAN/MERGEABLE. Cargo and exact-head review remain
      delegated; the pre-fix head and candidate are invalid.
- [x] #192 final exact-head subagent review PASS on
      `84746f9e23a277411905907c345bf7bede0e1841`: `Arc`/`Mutex` imports are
      present, all four repo tests and retry behavior remain intact, and module
      boundaries are valid. No compile or behavior blocker was found. Subagent
      did not run Cargo or modify the worktree; coordinator `git diff --check`
      remains clean.
- [x] PR #193 `codex/cord-50-cli-attachment-tests` is the next bounded child of
      #192: base branch `codex/cord-50-cli-repo-tests` at
      `84746f9e23a277411905907c345bf7bede0e1841`, exact head
      `1c0aeba057baf6a62820a7dfd98e7f3a227f5d4a`, tree
      `b47e335e15505b35b30895a7520eba283e5ee025`, candidate
      `823b4a08a741a3b3ccb5092a8f78e6faaf7ef454`. It extracts the attachment
      upload/download multipart, path-safety, and output contract test into
      `attachment_command_tests.rs`; task-token fields, filename sanitization,
      and destination behavior are unchanged. The exact-head static review found
      and fixed a missing `std::fs` import in `1c0aeba0`; scoped diff-check passes
      and PR #193 is Ready (`isDraft=false`),
      CLEAN/MERGEABLE. Cargo and exact-head review remain delegated.
- [x] #193 final exact-head subagent review PASS on
      `1c0aeba057baf6a62820a7dfd98e7f3a227f5d4a`: `std::fs` import is present;
      upload/download, multipart, path-safety, output assertions, and module
      boundary are intact. No compile or behavior blocker was found. Subagent
      did not run Cargo or modify the worktree; coordinator `git diff --check`
      remains clean.
- [x] PR #194 `codex/cord-50-cli-project-tests` is the next bounded child of #193:
      base branch `codex/cord-50-cli-attachment-tests` at
      `1c0aeba057baf6a62820a7dfd98e7f3a227f5d4a`, exact head
      `5eb4da32ceb96a0b5be297d8a322f4f1e12c8925`, tree
      `e852f0c9431f30d93f87ab9ed759895fcdaea304`, candidate
      `f7c1e0ad3888310758767d2932f60d465185477c`. It extracts five project
      list/get/create/status parser, HTTP, resource-bundle, and table contract
      tests into `project_command_tests.rs`; status validation, prefix resolution,
      payloads, and Go-compatible output are unchanged. Scoped rustfmt and
      `git diff --check` pass; PR #194 is Ready (`isDraft=false`), MERGEABLE
      (checks settling). Cargo and exact-head review remain delegated.
- [x] #194 exact-head subagent review PASS on
      `5eb4da32ceb96a0b5be297d8a322f4f1e12c8925`: all five project tests are
      preserved, project-resource tests remain untouched, imports and routing
      are correct, and status/resource payload behavior plus module boundaries
      are intact. No compile or behavior blocker was found. Subagent did not run
      Cargo or modify the worktree; coordinator `git diff --check` remains clean.
- [x] PR #195 `codex/cord-50-cli-project-resource-tests` is the next bounded
      child of #194: base branch `codex/cord-50-cli-project-tests` at
      `5eb4da32ceb96a0b5be297d8a322f4f1e12c8925`, exact head
      `0a0a45a719a374a961c4c645294bd79e1163eef7`, tree
      `e5fa9d23eb14ad980f7f2d903e5c8b46a7c47f9e`, candidate
      `5e8a5d708c240529b82477c2b6d4e8bbafd9d4eb`. It extracts four project-
      resource add/list/update/remove parser and HTTP contract tests into
      `project_resource_command_tests.rs`; opaque refs, clear flags, prefix
      resolution, and output behavior are unchanged. Scoped rustfmt and
      `git diff --check` pass; PR #195 is Ready (`isDraft=false`),
      CLEAN/MERGEABLE. Cargo and exact-head review remain delegated.
- [x] #195 exact-head subagent review PASS on
      `0a0a45a719a374a961c4c645294bd79e1163eef7`: all four project-resource
      tests are preserved, routing and opaque-ref/clear/prefix/output behavior
      remain intact, and imports/module boundary are valid. No compile or
      behavior blocker was found. Subagent did not run Cargo or modify the
      worktree; coordinator `git diff --check` remains clean.
- [x] PR #196 `codex/cord-50-cli-config-tests` is the next bounded child of #195:
      base branch `codex/cord-50-cli-project-resource-tests` at
      `0a0a45a719a374a961c4c645294bd79e1163eef7`, exact head
      `631ade755b43cd733341e0154b823b3eb1f9473c`, tree
      `c37041befdfb22e3acbe1e389a68ebea33e6eb4d`, candidate
      `2a01861662c247d4563430afe08dcd4965e5cf27`. It extracts five config
      show/set parser, persistence, validation, profile-scoping, redaction, and
      task-local guard contract tests into `config_command_tests.rs`; production
      behavior is unchanged. Scoped rustfmt and `git diff --check` pass; PR #196
      is Ready (`isDraft=false`), MERGEABLE (checks settling). Cargo and
      exact-head review remain delegated.
- [x] #196 exact-head subagent review PASS on
      `631ade755b43cd733341e0154b823b3eb1f9473c`: all five config tests are
      preserved; fs/Path imports, profile scoping, redaction, and task-local
      fail-closed behavior remain covered. No compile or behavior blocker was
      found. Subagent did not run Cargo or modify the worktree; coordinator
      `git diff --check` remains clean.
- [x] PR #197 `codex/cord-50-cli-auth-tests` is the next bounded child of #196:
      base branch `codex/cord-50-cli-config-tests` at
      `631ade755b43cd733341e0154b823b3eb1f9473c`, exact head
      `269a57eda39d0c7ed6e1b767a6b47e2aa211f4e5`, tree
      `d1ce738a099580dc9128afa087fa45b8cfdf53df`, candidate
      `62f49f74324840e0b66bcb015b32e32243c3a67d`. It extracts three auth
      status/logout parser, API, redaction, profile-scoping, and task-guard
      contract tests into `auth_command_tests.rs`; production behavior is
      unchanged. Scoped rustfmt and `git diff --check` pass; PR #197 is Ready
      (`isDraft=false`), MERGEABLE (checks settling). Cargo and exact-head
      review remain delegated.
- [x] #197 exact-head subagent review PASS on
      `269a57eda39d0c7ed6e1b767a6b47e2aa211f4e5`: all three auth status/logout
      tests are preserved; imports/routes, token redaction, task guards, and
      profile-scoped logout behavior remain covered. No compile or behavior
      blocker was found. Subagent did not run Cargo or modify the worktree;
      coordinator `git diff --check` remains clean.
- [x] #172 exact-head subagent review PASS: UpdateArgs retains `parse_cli_duration`,
      timeout help and `Duration` semantics; VersionOutput defaults/ValueEnum/visibility,
      handlers, dispatch, and tests have no compile/behavior blocker. Cargo remains
      delegated.
- [x] Next structural slice #172, `codex/cord-50-cli-root-schema`, is based on
      #171 at `cc7b8d569873a240596976e0ea2f69aff5a921e9`; exact head
      `192b888c3f4caca725c1eff26503207e96acbadf`, tree
      `ba085eddc3bce0a217e4e46ea11c8cfb64739ff2`, candidate
      `5774f6bf72ba332ecc2dc2dc8795ac479c98ec1e`. It moves only UpdateArgs and
      VersionOutput to `root_command_schema.rs`; update/version execution is unchanged.
      New schema rustfmt and `git diff --check` pass; Cargo/review/gate are delegated
      and have not been run by this thread. PR is Ready (`isDraft=false`).
- [x] Next structural slice #171, `codex/cord-50-cli-setup-schema`, is based on
      #170 at `b6e5f690b42c15e3dc8e008b6a2843549eba828e`; exact head
      `cc7b8d569873a240596976e0ea2f69aff5a921e9`, tree
      `d705d036d7ce19576495cb41af38850939ee4d11`, candidate
      `1234944e1f776f2fc8aa39ef06dabc151f7cd335`. It moves only Setup Cloud/self-host
      parser types and error classification to `setup_command_schema.rs`; setup
      execution/profile persistence is unchanged. New schema rustfmt and
      `git diff --check` pass; Cargo/review/gate are delegated and have not been run
      by this thread. PR is Ready (`isDraft=false`).
- [x] #171 exact-head subagent review PASS: Setup callback/app URL/port defaults,
      help text, `HealthProbeError` visibility, `SetupError`, migrated argument
      visibility, handlers, dispatch, and tests have no compile/behavior blocker.
      Cargo remains delegated.
- [x] #170 exact-head subagent review PASS: squad/member/activity clap fields,
      defaults, help text, member type/role semantics, re-exports, handlers,
      dispatch, and tests have no compile/behavior blocker. Cargo remains delegated.
- [x] #169 exact-head subagent review PASS: workspace/member/MCP/create/update clap
      fields, defaults, help text, `PathBuf`/stdin semantics, re-exports, handlers,
      dispatch, and tests have no compile/behavior blocker. Cargo remains delegated.
- [x] Next structural slice #170, `codex/cord-50-cli-squad-schema`, is based on
      #169 at `145866c38861194779e3314f5859162e40fc807b`; exact head
      `b6e5f690b42c15e3dc8e008b6a2843549eba828e`, tree
      `66fab2aa14d807b589f30cdef8f635720b847fb8`, candidate
      `dca672fa455dc4de17f414122b09c2d939efeac2`. It moves only squad/member/activity
      parser types to `squad_command_schema.rs`; execution behavior is unchanged.
      New schema rustfmt and `git diff --check` pass; Cargo/review/gate are delegated
      and have not been run by this thread. PR is Ready (`isDraft=false`).
- [x] After the replacement PRs were created and exact refs verified, #159 was closed
      as `SUPERSEDED` without deleting or rewriting its branch. Its review/gate
      evidence is not reused by any replacement PR.

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
- [x] Added `cordy skill files list <skill-id>` to PR #129. It fetches the
      encoded skill-file endpoint, preserves the Go table columns, keeps JSON
      objects intact, and intentionally omits sensitive file content from
      table output. Exact head is
      `ef5cc6370c2bce1cc4fc1fa3b0cee0bce9bde098`, parent
      `5121aaad75fb7dcb8150a4bf48cd0eae812078ba`, tree
      `08b2fbb94edc3d26837fa767b5983fead4ad2131`, candidate
      `c6880d6fe033fe3d62233f992a1fc3928733986e`, base
      `a4fbdd040bd8de34ee780fd1e5407bab8cceb17c`; remote head and PR
      Ready (`draft=false`) were verified. Scoped rustfmt/diff-check pass;
      Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill files upsert <skill-id>` to PR #129. It validates the
      required path/content, preserves mutually exclusive UTF-8 content
      sources, sends the Go-compatible PUT body, and renders JSON or table
      output. Exact head is
      `b4cda54b7228f65eb421d29320fc6794209dfc03`, parent
      `ef5cc6370c2bce1cc4fc1fa3b0cee0bce9bde098`, tree
      `b549ef34993666507733ae4c9385ef30384b1caa`; the PR base had externally
      advanced to `f78aa57fb09e8db0d6cc79c823658d0789cdb3ed` and GitHub
      temporarily reported `candidate=null`, `mergeable=dirty` (restack is
      delegated to pro). Remote head and `draft=false` were verified. Scoped
      rustfmt/diff-check pass; Cargo/review/gate/merge remain with pro.
- [x] Added `cordy skill files delete <skill-id> <file-id>` to PR #129. It
      encodes both path segments, preserves authenticated workspace headers,
      and matches Go's 204 success output with empty-ID guards. The slice
      commit is `206e6f1e`; during push, pro added daemon auth and cumulative
      main/base commits, so the current non-rewrite-synced PR head is
      `dc7b9843ecede29df3f25697b7656407ca8b21e2`, parents
      `9950f3d270118d7f011745ceb5c21ef5127dde07` +
      `c60582804597d2b94b90c8621fa3983d5c506b1a`, tree
      `46e43bb50364224a7431d7e8743c243a886dc317`, candidate
      `3d5dfe0f1fd010310941344a562587efb5c5daeb`, base
      `f78aa57fb09e8db0d6cc79c823658d0789cdb3ed`; remote head, `draft=false`,
      and CLEAN/MERGEABLE were verified. Scoped rustfmt/diff-check pass;
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
- [x] Added the missing Go-compatible `cordy autopilot trigger-rotate-url
      <autopilot-id> <trigger-id>` CLI slice on the cumulative daemon/CLI
      branch. It resolves UUID/prefix references inside the authenticated
      workspace, requires explicit confirmation unless `-y/--yes` is passed,
      calls the existing server rotate-webhook-token endpoint, and preserves
      Go's JSON/table webhook URL output without changing server rotation
      logic. Checkpoint `b74502316670a99925fc24b3e27617d31a4516b8`, parent
      `12080108c9764cf5024d2a639839684cdff49211`, tree
      `b26add6af1d2e6756ac11a532fffbe601c96387f`; scoped rustfmt and
      `git diff --check` pass, Cargo/review/gate/merge remain with pro. PR
      #129 was already merged at head `62b28e4a`; this post-merge checkpoint
      is pushed on the cumulative branch and awaits pro's new Ready PR
      candidate (no stale candidate is reused).
- [x] Ported the real interactive/browser `cordy login` flow and removed the
      setup-only “interactive login unavailable” gap on the cumulative branch
      `codex/cord-50-daemon-command-assembly`. Feature commit
      `ef75d3080367ca9d7f56a9d1719662f5ba70d463` (parent
      `b74502316670a99925fc24b3e27617d31a4516b8`, tree
      `4f01427e7bef0c266a11d86dfbabf80b28e3aae9`) adds the bounded local
      callback server, random state validation, browser fallback, JWT→PAT
      exchange and verification, authenticated workspace discovery, human/
      daemon guard, and atomic profile credential/workspace reset. Follow-up
      `780aa3d0` validates HTTP(S) app URLs. Scoped rustfmt and
      `git diff --check` pass; Cargo/review/gate/merge are intentionally owned
      by pro. Branch was pushed; no PR/candidate is created or reused here.
- [x] Completed the adjacent login workspace contract in
      `a39be40d6034decaa7c96396f8c76de395dd5fff` (parent `780aa3d0`, tree
      `ee5a655a9de55a5795dc6c39778dc18c24271e9c`). When the authenticated user
      has no workspace, Rust now opens a safe `/workspaces/new` URL, polls the
      authenticated API for at most five minutes, writes the discovered default
      workspace atomically, and reports timeout/discovery failures without
      credentials. Scoped rustfmt and `git diff --check` pass; pushed on the
      same cumulative branch for pro's Ready PR handling; Cargo/review/gate/
      merge remain with pro.
- [x] Fixed a concrete daemon lifecycle boundary in
      `8512f58720307ca0dcc8f0c79e07fa6a7039d122` (parent `a39be40d`, tree
      `e8769c86102c1c1e59d2a7f570650f84fd8a2344`): Rust `daemon stop` now
      applies the existing human-local guard before any host daemon control,
      matching Go and preventing task/daemon-managed contexts from reaching
      stop logic. Added a regression test; scoped rustfmt and diff-check pass.
      Pushed on the cumulative branch; Cargo/review/PR/merge remain with pro.
- [x] Fixed the matching restart boundary in
      `d4e935b7aeefaf4e5a40a544c7bc91719f61ec5d` (parent `8512f587`, tree
      `cdc9235e9004cf5174f4de457a61bc0c62fd7fa9`): Rust `daemon restart` now
      rejects daemon/task-managed contexts before host lifecycle control, with
      a regression test. Scoped rustfmt and diff-check pass; pushed on the
      cumulative branch; Cargo/review/PR/merge remain with pro.
- [x] Closed the remaining root-form setup flag gap in
      `3bc22991b0a233d8fa523d29854513d971f22eba` (parent `b44b1dbc`, tree
      `adb20fc2bbf6386b8440281e87e8065b5185df97`). Plain `cordy setup
      --callback-host` now matches Go and is propagated to browser login, with
      parser/precedence tests. Scoped rustfmt and diff-check pass; pushed on
      the cumulative branch; Cargo/review/PR/merge remain with pro.
- [x] Completed setup callback-host parity in
      `b44b1dbcc0fd34b2caebd093abc9d1d31f3432b0` (parent `d4e935b7`, tree
      `868114c31af566633937bc79dc7fb7bbca503486`). `setup cloud` and
      `setup self-host` now expose and propagate the Go-compatible callback
      host to browser login, with parser/propagation tests. Scoped rustfmt for
      changed files and diff-check pass (an unrelated existing `daemon.rs`
      formatting hunk remains untouched); pushed on the cumulative branch.
      Cargo/review/PR/merge remain with pro.
- [ ] Audit Go-only background workers, schedulers, reconcilers, event side effects,
      Redis behavior, metrics, and shutdown lifecycle; implement each missing Rust
      production path in the current thread.
- [x] Bounded audit of CLI/daemon lifecycle and Slack/Telegram channel runtime
      found no additional P0/P1, ownership, media, cancellation, shutdown, or
      changed-code compile gap after the callback-host fixes. No P2/style-only
      changes were added. Current cumulative implementation branch is pushed at
      `3bc22991b0a233d8fa523d29854513d971f22eba` (tree
      `adb20fc2bbf6386b8440281e87e8065b5185df97`); pro owns creation of its
      Ready PR, review, Cargo gate, and merge.
- [x] Phase 4 repository/VCS audit completed against the Go handlers and Rust
      `vcs.rs`/repo checkout paths. Workspace/member and daemon-token binding,
      secret/webhook handling, signature/body limits, out-of-order protection,
      checkout containment/locking/cancellation, and bounded git operations
      are covered. Remaining JSON-shape/trim differences are non-P1; no code
      or Cargo change was justified.
- [x] Workspace/member route audit completed: authentication, role and
      task-token workspace fences, transactional create/delete/leave cleanup,
      owner constraints, bounded workspace deletion locks, invitation state,
      Redis admission, cache invalidation, events, and runtime/task side
      effects match the Go contract. No P0/P1 or changed-code compile gap;
      no implementation change was justified.
- [x] Fixed a real P1 in comment task lifecycle at
      `804a3d11ce47d9c63e871e02e60680b9421d930f` (parent `3bc22991`, tree
      `7ae60a0a71d21c26ae45fe247ac86c5875770f27`). Comment update/delete now
      replays surviving coalesced trigger comments after cancellation, grouped
      by agent/task metadata and ordered by creation time; mutation failures
      restore the complete batch, while deleted/edited triggers are excluded
      only after success. Delegated recovery and note comments remain filtered;
      tests cover exclusion, restoration, deduplication, and note behavior.
      Scoped rustfmt/diff-check pass; pushed for pro's Ready PR/gate/merge.
- [x] Follow-up issue mutation audit found no matching survivor gap: issue
      status/reassignment updates do not cancel existing tasks in either
      implementation; issue deletion cancels tasks before deleting all
      comments, so no live survivors require replay. Transaction, attachment
      cleanup, event, and fail-closed cancellation boundaries remain intact.
- [x] Autopilot/webhook route and worker audit completed: workspace/creator
      guards, trigger/delivery ownership, token rotation/revocation, ingress
      body/signature/dedupe handling, Redis admission and Retry-After, lease
      retries, paused/disabled/quota terminal states, replay idempotency, and
      bounded shutdown match Go. No additional P0/P1 or changed-code blocker;
      no implementation change was justified.
- [x] Notification/event/realtime side-effect audit completed: ordered
      post-commit publication, FIFO/panic containment, workspace/personal
      recipient scope, sensitive-field filtering, Redis relay dedupe/retry,
      lease recovery, client backpressure, canonical-state reconnect, and
      bounded producer-to-consumer shutdown match Go. No new P0/P1 or
      changed-code blocker; no implementation change was justified.
- [x] Task claim/retry lifecycle audit completed: `SKIP LOCKED` runtime and
      heartbeat ownership, stale reclaim/lease refresh, CAS finalization,
      terminal idempotence, retry ceilings/backoff, successor dedupe, orphan
      recovery, and failure/event reconciliation match Go. No new P0/P1 or
      changed-code blocker; no implementation change was justified.
- [x] Attachment/media route and channel-adapter audit completed: membership
      before upload/download/presign, task/agent scope, key containment,
      bounded streaming and cancellation cleanup, signed capability scope,
      range/content-disposition/SVG handling, orphan semantics, and media-ledger
      lease/tombstone cleanup match Go. No new P0/P1 or changed-code blocker;
      no implementation change was justified.
- [x] Auth/token/OAuth audit completed: trusted proxy identity, OTP CAS and
      attempt budget, PAT/Cloud PAT owner/disabled/revoke/cache semantics,
      bounded Cloud PAT transport, OAuth callback state behavior, human/machine
      guards, and sensitive logging match the current Go contract. No new P0/P1
      or changed-code blocker; no implementation change was justified.
- [x] Fixed a provider-session P1 in
      `c3822514ae7cd91b0d7e7fdfe278826f21af51e2` (parent `804a3d11`, tree
      `d7d888da236841ae9bd5a7d2b47a074cfd0edce6`). Codex sessions now defer
      `pin_task_session` until the corresponding persisted rollout is visible,
      polling at a bounded 50ms interval; non-Codex providers retain immediate
      pinning. Added persisted/missing rollout tests. Scoped rustfmt and
      diff-check pass; pushed for pro's Ready PR/gate/merge.
- [x] Follow-up race audit of `c3822514` found no new issue: cancellation and
      drain timeout remain bounded, the first non-empty session ID is retained,
      non-Codex providers pin unchanged, persisted rollout pins once, and pin
      failures remain best-effort without duplicate side effects.
- [x] Fixed a squad/agent authorization P1 in
      `c3e9db3cae29d79f1e41ae9d0dd441642d4a5a88` (parent `c3822514`, tree
      `242fa90a487957d0b223d4a882d92c377b614f4f`). Autopilot squad-leader
      validation now uses a dedicated member invoke gate rather than the
      broader inspect/access gate, so workspace admins cannot run another
      member's private agent. Added owner/private regression coverage; scoped
      rustfmt and diff-check pass; pushed for pro's Ready PR/gate/merge.
- [x] Global invoke-gate follow-up found no second misuse: chat, issue,
      comment, quick-action, squad, and autopilot run/enqueue paths use the
      member invoke gate; remaining broader access calls are inspect/cancel/
      pin/list or wiring paths matching Go. No further code change justified.
- [x] Project/property/issue-status audit completed: workspace and role
      fences, transactional/locked project deletion, property type/config and
      membership validation, advisory-lock writes, post-commit events, status
      feature gates, archive/reorder locks, and status race handling match Go.
      No new P0/P1 or changed-code blocker; no implementation change justified.
- [x] Chat/session route audit completed: workspace/creator and invoke fences,
      transactional session/project binding, ordered cleanup, queued-task
      coalescing/cancel, late-response/archive handling, message/attachment
      persistence, post-commit notifications, and shutdown reconciliation match
      Go. No new P0/P1 or changed-code blocker; no implementation change.
- [ ] CLI login structural refactor is pushed at
      `154df0b015a9a23ecd9c07720a5a9ffc6215ce93` (parent
      `a0cbc453a3b2fe94841b70a34c28de53bdb0124c`, tree
      `a85cc0df438a194ae70c586d0087f65260d0659a`): browser callback, state
      validation, workspace discovery, and URL builders now live in
      `cordy-cli/src/login.rs`; command/profile orchestration remains in
      `lib.rs`. The serial subagent reviewed the exact refactor and fixed
      test-only module visibility/import omissions; diff-check passed, no
      Cargo was run, and the subagent could not rerun rustfmt because the
      command was unavailable. Pro owns Ready PR, Cargo gate, and merge.
- [ ] CLI daemon lifecycle structural refactor is pushed at
      `16540c59f974b74d9d6efdfca92b45fc33e966f0` (parent
      `154df0b015a9a23ecd9c07720a5a9ffc6215ce93`, tree
      `7e16b07aa89e890255063d813d307012c58a6ec8`): setup-after-daemon, start,
      restart, and stop now live in `cordy-cli/src/daemon_commands.rs`;
      parsing, setup policy, and status rendering remain in `lib.rs`. Scoped
      rustfmt and diff-check pass; no Cargo was run. The serial subagent
      reviewed exact head `16540c59` and found no issue (environment lacked a
      rustfmt binary for a second pass); pro owns Ready PR, gate, and merge.
- [ ] CLI daemon status/log structural refactor is pushed at
      `476273172ee933d60bf14c328c2099ece8954288` (parent
      `16540c59f974b74d9d6efdfca92b45fc33e966f0`, tree
      `2b8dfc49392cf7ad63ae91abdda1fd71fd6114852`): profile discovery, health
      status rendering, log tail/follow, and their bounded parsers now share
      `cordy-cli/src/daemon_commands.rs`. Scoped rustfmt/diff-check pass; no
      Cargo was run. The serial subagent reviewed exact head `47627317` and
      found no issue (environment lacked a rustfmt binary for a second pass);
      pro owns Ready PR, gate, and merge.
- [ ] CLI daemon diagnostics entry refactor is pushed at
      `cce3266f23b5b4b5f514a5a9c1fa2452fd995e16` (parent
      `476273172ee933d60bf14c328c2099ece8954288`, tree
      `1f9b1916bd1b01fc01ee39873ebcb30a8993d2d9`): probe-runtimes and
      disk-usage command orchestration now route through
      `cordy-cli/src/daemon_commands.rs`; scanning/formatting helpers remain
      in `lib.rs` for a separate slice. Scoped rustfmt/diff-check pass; no
      Cargo was run. The serial subagent reviewed exact head `cce3266f` and
      found no issue (environment lacked a rustfmt binary for a second pass);
      pro owns Ready PR, gate, and merge.
- [ ] CLI auth status/logout structural refactor is pushed at
      `971708260c1f532e8a9a4f4bacf8639671edbbfa` (parent
      `cce3266f23b5b4b5f514a5a9c1fa2452fd995e16`, tree
      `c4e5479c892aeacc6cdd23d95de7d3ac58ee9a3b`): status output, logout, and
      credential resolution now live in `cordy-cli/src/auth_commands.rs`;
      task-local guard remains shared in `lib.rs`. Scoped rustfmt/diff-check
      pass; no Cargo was run. The serial subagent reviewed exact head
      `97170826` and found no issue (environment lacked a rustfmt binary for a
      second pass); pro owns Ready PR, gate, and merge.
- [ ] CLI setup orchestration structural refactor is pushed at
      `9f1a68e4144fb8dbe148e6e8b04addd95613085a` (parent
      `971708260c1f532e8a9a4f4bacf8639671edbbfa`, tree
      `ffbbbf9da9714093e05dc8bf8fb576684c22a012`): confirmation, profile
      resolution, health-before-write, daemon action policy, and login/daemon
      handoff now live in `cordy-cli/src/setup_commands.rs`. Scoped
      rustfmt/diff-check pass; no Cargo was run. The serial subagent reviewed
      exact head `9f1a68e4` and found no issue (environment lacked a rustfmt
      binary for a second pass); pro owns Ready PR, gate, and merge.
- [ ] CLI update policy structural refactor is pushed at
      `9f8451e2083cb6ce847326855c4540aa534a73e6` (parent
      `9f1a68e4144fb8dbe148e6e8b04addd95613085a`, tree
      `a7c2f715c3cfc97a73300501e82b41ddc6d96205`): timeout validation, update
      error redaction, and presentation now live in
      `cordy-cli/src/update_commands.rs`; daemon detection/download/install
      remains behind its typed facade. Scoped rustfmt/diff-check pass; no
      Cargo was run. The serial subagent reviewed exact head `9f8451e2` and
      found no issue (environment lacked a rustfmt binary for a second pass);
      pro owns Ready PR, gate, and merge.
- [ ] CLI config command policy structural refactor is committed at
      `4d7e4f049d5c6e7f84b48d4d4ea6386a5cd5a45e` (parent
      `9f8451e2083cb6ce847326855c4540aa534a73e6`, tree
      `838254872a3df6cbafcd9763fa0a1aa5d51b9154`): config show/set handlers,
      output rendering, supported-key validation, URL/path validation, and
      Go duration/bool parsing now live in `cordy-cli/src/config_commands.rs`;
      dispatch and existing test references remain stable. Scoped rustfmt and
      diff-check pass; no Cargo was run. The serial subagent reviewed exact
      head `4d7e4f04` and found no issue; pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI disk-usage rendering structural refactor is committed at
      `0f79c830f7c87395ef0f5ad1957490be96a0ec15` (parent
      `4d7e4f049d5c6e7f84b48d4d4ea6386a5cd5a45e`, tree
      `80df9ca56ea5da18be28c7cd73700887099812f6`): report/aggregate tables,
      byte/ratio/age formatting, repository-cache lines, and status warnings
      now live in `cordy-cli/src/disk_usage_output.rs`; scanning, validation,
      parent-status resolution, and dispatch remain unchanged. Scoped rustfmt
      and diff-check pass; no Cargo was run. The serial subagent reviewed exact
      head `0f79c830` and found no issue; pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI disk-usage policy structural refactor is committed at
      `e3ee3e8740ddf0daa4b545a68f8e042d3c9d6a53` (parent
      `0f79c8306cc6518226af08a176fb62e6f419d543`, tree
      `3b67687ef1308ac7c2853a7fdebdfdfbe6b4bda8`): task-context validation,
      profile/root resolution, profile enumeration, top limits, and parent
      status lookup now live in `cordy-cli/src/disk_usage_commands.rs`;
      dispatch and disk scanning remain unchanged. Scoped rustfmt and
      diff-check pass; no Cargo was run. The serial subagent reviewed exact
      head `e3ee3e87` and found no issue; pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI runtime rows/profile output structural refactor is committed at
      `e39dcbaab46abdeb7c421af63106746b0179c4f8` (parent
      `e3ee3e8740ddf0daa4b545a68f8e042d3c9d6a53`, tree
      `b6530e7d5be56e32989c425875f7406fe86ea918`): runtime usage/activity
      rows and runtime-profile JSON/table rendering now live in
      `cordy-cli/src/runtime_output.rs`; API/update/profile policy remains in
      `lib.rs`. Scoped rustfmt and diff-check pass; no Cargo was run. The
      serial subagent reviewed exact head `e39dcbaa` and found no issue; pro
      owns push/Ready PR, gate, and merge.
- [ ] CLI runtime-delete policy structural refactor is committed at
      `0e5533b33f837cb14e5e0eb7fcd720519946614b` (parent
      `e39dcbaab46abdeb7c421af63106746b0179c4f8`, tree
      `6724e3a81985100b1d1dce9cce4a36d699dbfbca`): active-agent 409 conflict
      decoding, cascade display data, and delete result presentation now live
      in `cordy-cli/src/runtime_delete.rs`; delete request and cascade API
      policy remain unchanged. Scoped rustfmt and diff-check pass; no Cargo
      was run. The serial subagent reviewed exact head `0e5533b3` and found no
      issue; pro owns push/Ready PR, gate, and merge.
- [ ] CLI runtime-update policy structural refactor is committed at
      `7bc594ee612abee02dc81b627eefd2d182221cc2` (parent
      `0e5533b33f837cb14e5e0eb7fcd720519946614b`, tree
      `a395a52380a62fa7b331a5b763406b1b552d8242`): target validation,
      bounded polling, request timeout selection, terminal status handling,
      and table/JSON output now live in `cordy-cli/src/runtime_update.rs`;
      runtime API dispatch remains unchanged. Scoped rustfmt and diff-check
      pass; no Cargo was run. The serial subagent reviewed exact head
      `7bc594ee` and found no issue; pro owns push/Ready PR, gate, and merge.
- [ ] CLI runtime-profile command structural refactor is committed at
      `03c9c43bf2dc6868010b8b52c3c7e3bb62366a1f` (parent
      `7bc594ee612abee02dc81b627eefd2d182221cc2`, tree
      `d9bb6548041db709136e140b0a1408b126813cf5`): protocol-family and path
      validation, profile CRUD, 409 mapping, and local path overrides now
      live in `cordy-cli/src/runtime_profile.rs`; command dispatch stays
      unchanged. Scoped rustfmt and diff-check pass; no Cargo was run. The
      serial subagent reviewed exact head `03c9c43b` and found no issue; pro
      owns push/Ready PR, gate, and merge.
- [ ] CLI autopilot output structural refactor is committed at
      `32ec4bb24563601cafcabc3d06b9d86984fa32b5` (parent
      `03c9c43bf2dc6868010b8b52c3c7e3bb62366a1f`, tree
      `84018510cc69b424820557e3fe69c89bf81efdac`): run/list tables, assignee
      display and webhook URL fallback now live in
      `cordy-cli/src/autopilot_output.rs`; autopilot API/resolver logic stays
      unchanged. Scoped rustfmt and diff-check pass; no Cargo was run. The
      serial subagent reviewed exact head `32ec4bb2` and found no issue; pro
      owns push/Ready PR, gate, and merge.
- [ ] CLI autopilot resolver structural refactor is committed at
      `5f5ab9337e0ce5da1c53d4b3b1d7c91d1de00dec` (parent
      `32ec4bb24563601cafcabc3d06b9d86984fa32b5`, tree
      `caeaa7504377b5c7de80a91cc3d10dff45026ffa`): UUID/prefix pagination,
      agent/member/subscriber resolution, deduplication, and assignee-name
      loading now live in `cordy-cli/src/autopilot_resolver.rs`; API command
      policy remains unchanged. Scoped rustfmt and diff-check pass; no Cargo
      was run. The serial subagent reviewed exact head `5f5ab933` and found no
      issue; pro owns push/Ready PR, gate, and merge.

- [ ] CLI repository command structural refactor is committed at
      `b4e6752d7171aea7bc6f784931707fd151246032` (parent
      `5f5ab9337e0ce5da1c53d4b3b1d7c91d1de00dec`, tree
      `6f9799933541406e486fe0da55bc258c28c5e4ca`), with the serial subagent's
      compile-visibility fix at `0471d51056cf94c29a5ee0909d55d608ef2174a9`
      (tree `cd7176d020ce6d0d5e111f1c5bccdae398b1018f`). Repository list/add/
      remove/checkout policy, registry DTOs, URL normalization, and daemon
      checkout retry handling now live in `cordy-cli/src/repo_commands.rs`;
      dispatch and parent tests retain their existing contract. Scoped rustfmt
      and diff-check pass; Cargo was not run. The serial subagent found and
      fixed the moved retry-helper visibility issue; pro owns push/Ready PR,
      gate, and merge.
- [ ] CLI chat/attachment command structural refactor is committed at
      `89f26842427ad24df568bbbe5a91d71ad2b2f8d2` (parent
      `0471d51056cf94c29a5ee0909d55d608ef2174a9`, tree
      `00a9ae650466ef64e8bc3bff85654df9d5ec2ca2`): chat history/thread query
      and table formatting plus attachment upload/download path, timeout, and
      output policy now live in `cordy-cli/src/chat_commands.rs`; parent
      dispatch and shared chat reply formatting remain unchanged. Scoped
      rustfmt and diff-check pass; Cargo was not run. The serial subagent
      reviewed exact head `89f26842` and found no issue; pro owns push/Ready
      PR, gate, and merge.
- [ ] CLI skill command structural refactor is committed at
      `1684d444e6d7569e821a444c901145d13414d232` (parent
      `89f26842427ad24df568bbbe5a91d71ad2b2f8d2`, tree
      `394528df29f32343c963866db2108b736d5d434a`): skill CRUD,
      content-source validation, import/refresh/search, archive handling,
      skill-file mutations, and JSON/table renderers now live in
      `cordy-cli/src/skill_commands.rs`; dispatch and parent test references
      retain the existing contract. Scoped rustfmt and diff-check pass; Cargo
      was not run. The serial subagent reviewed exact head `1684d444` and
      found no issue; pro owns push/Ready PR, gate, and merge.
- [ ] CLI property/issue-property structural refactor is committed at
      `a04eef47a230b5feac50a80e56f96846915115da` (parent
      `1684d444e6d7569e821a444c901145d13414d232`, tree
      `df973c827ba74ab2ff3ce2ec25068c100c140290`): property DTOs,
      option and actor encoding, workspace property CRUD, issue-property
      mutation, and JSON/table rendering now live in
      `cordy-cli/src/property_commands.rs`; dispatch and parent tests retain
      their existing contract. Scoped rustfmt and diff-check pass; Cargo was
      not run. The serial subagent reviewed exact head `a04eef47` and found no
      issue; pro owns push/Ready PR, gate, and merge.
- [ ] CLI squad command structural refactor is committed at
      `639684cb7f4f0179f2d291c96a0a0c9a1322e2cd` (parent
      `a04eef47a230b5feac50a80e56f96846915115da`, tree
      `e3c5adc8661a6ac5dc61ff2303b339489c9e06f6`), with the serial subagent's
      test-visibility fix at `06d85e22384389403003ba75caf11b5954c8f967`
      (tree `b983162642dfa545ea874f0e2cf61b2c08d49db8`). Squad CRUD, member
      role/membership operations, activity recording, and table output now
      live in `cordy-cli/src/squad_commands.rs`; dispatch and parent tests keep
      their existing contract. Scoped rustfmt and diff-check pass; Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI workspace MCP command structural refactor is committed at
      `53bd13f7879eb9188c18ba69137579dadc8e3115` (parent
      `06d85e22384389403003ba75caf11b5954c8f967`, tree
      `985006c482c3878910f773e285fcd61af00748a9`): MCP DTOs, config input
      parsing, list/add/update/remove operations, and table/JSON rendering now
      live in `cordy-cli/src/workspace_mcp_commands.rs`; agent MCP shared
      references and parent dispatch remain unchanged. Scoped rustfmt and
      diff-check pass; Cargo was not run. The serial subagent reviewed exact
      head `53bd13f7` and found no issue; pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI workspace command structural refactor is committed at
      `e84766ace06d96b04d25ad9fb1f9026fbe0ff62d` (parent
      `6b676579c44fc6c20df543a875be03beb4a1b8e4`, tree
      `8c938a017d734ca496bd4c7f477845ce261c0c34`): workspace list/get/create,
      update/switch/member operations, input resolution, workspace reference
      resolution, and table rendering now live in
      `cordy-cli/src/workspace_commands.rs`; parent dispatch and tests retain
      their existing contracts. The serial subagent removed duplicate parent
      renderers that would have blocked compilation. Scoped rustfmt and
      diff-check pass; Cargo was not run. Pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI user profile command structural refactor is committed at
      `47c86172b29ffda6cfd954a8c3f76d5cb89e48d5` (parent
      `e84766ace06d96b04d25ad9fb1f9026fbe0ff62d`, tree
      `46957236e2d12f4730cb0dc305f98907d9b716a8`): profile get/update,
      description input resolution, file-boundary handling, clear semantics,
      and table/JSON rendering now live in `cordy-cli/src/user_commands.rs`;
      parent dispatch and tests retain their existing contracts. The serial
      subagent reviewed exact head `47c86172` and found no issue. Scoped
      rustfmt/diff-check pass; Cargo was not run. Pro owns push/Ready PR, gate,
      and merge.
- [ ] CLI label command structural refactor is committed at
      `335954bb30849b66d24df4c5b4e8b95f0f305652` (parent
      `47c86172b29ffda6cfd954a8c3f76d5cb89e48d5`, tree
      `1ba50497795fc79510053d9e914022e219d3e081`): label list/get/create/update/
      delete and table/result rendering now live in
      `cordy-cli/src/label_commands.rs`; issue-label rendering reuses the
      extracted formatter through parent imports. The serial subagent reviewed
      exact head `335954bb` and found no issue. Scoped rustfmt/diff-check pass;
      Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI project core command structural refactor is committed at
      `252dce717766cfc532dc60a84a8fdfaf50299eb0` (parent
      `335954bb30849b66d24df4c5b4e8b95f0f305652`, tree
      `483345cd6515b0cdcebfa1ffc034e49616182370`): project list/get/create/
      update/delete/status, lead resolution, status validation, and output
      helpers now live in `cordy-cli/src/project_commands.rs`; project resource
      commands remain isolated for a later slice. The serial subagent reviewed
      exact head `252dce71` and found no issue. Scoped rustfmt/diff-check pass;
      Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI project resource command structural refactor is committed at
      `b61682f0c8625235f9919e3d21d2382337430da4` (parent
      `252dce717766cfc532dc60a84a8fdfaf50299eb0`, tree
      `c144f49d56e54869dd8503905d8ac6b404979211`): resource list/add/update/
      remove, resource-ref parsing/merge, UUID resolution, and output helpers
      now live in `cordy-cli/src/project_resource_commands.rs`; parent dispatch
      and tests retain their contracts. The serial subagent reviewed exact head
      `b61682f0` and found no issue. Scoped rustfmt/diff-check pass; Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI agent core command structural refactor is committed at
      `0fa943d55bcdd14746bfab1b1e14b2a79beeeb5b` (parent
      `b61682f0c8625235f9919e3d21d2382337430da4`, tree
      `f558cb4a90ff995f818e778cfc8d1905535b1703`): agent list/get/create/
      update, archive/restore, tasks, and avatar operations now live in
      `cordy-cli/src/agent_commands.rs`; skills/env/MCP/copy remain for later
      slices. The serial subagent reviewed exact head `0fa943d5` and found no
      issue. Scoped rustfmt/diff-check pass; Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI agent auxiliary command structural refactor is committed at
      `0cb3602e6d7425cb71e56b2bf846ac20d909fd85` (parent
      `0fa943d55bcdd14746bfab1b1e14b2a79beeeb5b`, tree
      `a99e6bd26dbe86e40acde3f7f94e64d63cc5570a`): agent skills/env/MCP/copy
      commands, MCP action/path helpers, and their dispatch now live in
      `cordy-cli/src/agent_commands.rs`; shared secret/permission helpers stay
      in the parent. The serial subagent reviewed exact head `0cb3602e` and
      found no issue. Scoped rustfmt/diff-check pass; Cargo was not run. Pro
      owns push/Ready PR, gate, and merge.

- [ ] CLI issue-subscriber command structural refactor is committed at
      `715d4c04787e095374e0c2e66ca42cec787b2de1` (parent
      `0cb3602e6d7425cb71e56b2bf846ac20d909fd85`, tree
      `b99dcb01a46ac7fe6af08c9b6cfa536ada515726`): subscriber list,
      table rendering, subscribe, and unsubscribe flows now live in
      `cordy-cli/src/issue_subscriber_commands.rs`; parent dispatch and
      regression-test visibility remain unchanged. The serial subagent reviewed
      the exact head and found no compile-visibility or behavior issue. Scoped
      rustfmt and `git diff --check` pass (the subagent environment lacked a
      rustfmt executable); Cargo was not run. Pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI issue-label command structural refactor is committed at
      `8079370d2b2f26085f4274c1e4632607383c68ff` (parent
      `715d4c04787e095374e0c2e66ca42cec787b2de1`, tree
      `eed039219659570b14e1fc038973b990d06579d9`): issue label list,
      attach, detach, resolution, and output formatting now live in
      `cordy-cli/src/issue_label_commands.rs`; the shared issue-label
      extractor remains in the parent for workspace label reuse. The serial
      subagent reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-metadata command structural refactor is committed at
      `1beca9ca823ca268b3f2de016360d1a20d37d432` (parent
      `8079370d2b2f26085f4274c1e4632607383c68ff`, tree
      `88289b0db4ee391b2d3b4d69b046a1f9bfe42c04`): metadata parsing,
      table/JSON formatting, list/get/set/delete flows, and 404 handling now
      live in `cordy-cli/src/issue_metadata_commands.rs`; parent dispatch
      and existing parser/table tests retain their contracts. The serial
      subagent reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-timeline command structural refactor is committed at
      `0b54230b232aa05522e64ec40ab549ea8fb4b8c3` (parent
      `1beca9ca823ca268b3f2de016360d1a20d37d432`, tree
      `bc3d321a2af2e36eae8a869135fd6e7476ad56c9`): timeline filter parsing,
      server request/truncation handling, actor enrichment, detail rendering,
      and table output now live in `cordy-cli/src/issue_timeline_commands.rs`;
      parent dispatch and parser/filter/table regression tests retain their
      contracts. The serial subagent reviewed the exact head and found no
      issue. Scoped rustfmt and `git diff --check` pass (the subagent
      environment lacked a rustfmt executable); Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI issue-search command structural refactor is committed at
      `e33c60894fd63baec504c989f44e881a8534fc5f` (parent
      `0b54230b232aa05522e64ec40ab549ea8fb4b8c3`, tree
      `403bcb929cac6b21a96445c21fecb9e395aa1663`): issue search query
      serialization, request execution, JSON output, and table rendering now
      live in `cordy-cli/src/issue_search_commands.rs`; parent dispatch and
      formatter regression-test visibility remain unchanged. The serial
      subagent reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue task-run command structural refactor is committed at
      `0828bfabbaee6c8a51ac493001225eac30efca0b` (parent
      `e33c60894fd63baec504c989f44e881a8534fc5f`, tree
      `b65553b8d9d860692d23ed088173715a20e69d01`): issue runs,
      run-message listing, cancellation, task-scope resolution, and table
      renderers now live in `cordy-cli/src/issue_task_commands.rs`; parent
      dispatch and existing formatter tests retain their contracts. The serial
      subagent reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue usage command structural refactor is committed at
      `16812d6149062ecb312d84b41911e9793e99313b` (parent
      `0828bfabbaee6c8a51ac493001225eac30efca0b`, tree
      `df66d01621a4d41342bf5a033aa35a35420d5e85`): issue usage request and
      JSON/table rendering now live in `cordy-cli/src/issue_usage_commands.rs`;
      the shared metadata value formatter remains in the parent. The serial
      subagent reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue rerun command structural refactor is committed at
      `3d47bdddfd98777fb48ef60d510a12ea8612cf22` (parent
      `16812d6149062ecb312d84b41911e9793e99313b`, tree
      `a31fe81989c4d2cb8f0dc3fc756bb2e80cb8e46e`): rerun request,
      agent-name enrichment, and JSON/table output now live in
      `cordy-cli/src/issue_rerun_commands.rs`; parent dispatch retains its
      existing contract. The serial subagent reviewed the exact head and found
      no issue. Scoped rustfmt and `git diff --check` pass (the subagent
      environment lacked a rustfmt executable); Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI issue comment-list structural refactor is committed at
      `0dfec48c2dbb3bada853cda33c3c65d65b26c59a` (parent
      `3d47bdddfd98777fb48ef60d510a12ea8612cf22`, tree
      `d05df835769f335bc2ef017d20e42235c2a8b3a9`): comment list option
      validation, pagination/cursor handling, compact output, actor enrichment,
      and table rendering now live in
      `cordy-cli/src/issue_comment_list_commands.rs`; parent dispatch and
      formatter tests retain their contracts. The serial subagent reviewed the
      exact head and found no issue. Scoped rustfmt and `git diff --check`
      pass (the subagent environment lacked a rustfmt executable); Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue comment-add structural refactor is committed at
      `36fb32a187c583582c2e2c9f291b080d058edb22` (parent
      `0dfec48c2dbb3bada853cda33c3c65d65b26c59a`, tree
      `497ad4592f8d6b0e8d6fe893032364f69429b64f`): content source parsing,
      path safety, attachment upload, and comment creation now live in
      `cordy-cli/src/issue_comment_add_commands.rs`; the parent test helper
      remains available through `pub(super)`. The serial subagent reviewed the
      exact head and found no issue. Scoped rustfmt and `git diff --check`
      pass (the subagent environment lacked a rustfmt executable); Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue comment-mutation structural refactor is committed at
      `62f1594d92f55ff5d36989bed42ddacbf4753ade` (parent
      `36fb32a187c583582c2e2c9f291b080d058edb22`, tree
      `d6c0493ef15541bb34b4915a25cc13e78957ec2e`): comment delete,
      resolve, and unresolve requests now live in
      `cordy-cli/src/issue_comment_mutation_commands.rs`; URL encoding,
      methods, output, and error semantics are unchanged. The serial subagent
      reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-status command structural refactor is committed at
      `8904a9d78662fb212c45cf10d94975700559b3be` (parent
      `62f1594d92f55ff5d36989bed42ddacbf4753ade`, tree
      `9385d7bfaab7d28e54d8300298129a8b051ddcf6`): status validation
      dispatch, PUT request, and JSON/table/stderr output now live in
      `cordy-cli/src/issue_status_commands.rs`; shared validation remains in
      the parent for create/update reuse. The serial subagent reviewed the
      exact head and found no issue. Scoped rustfmt and `git diff --check`
      pass (the subagent environment lacked a rustfmt executable); Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-assignment command structural refactor is committed at
      `d016ad65a8bfd880ce49113ec545cabd2ff7c276` (parent
      `8904a9d78662fb212c45cf10d94975700559b3be`, tree
      `4d251d1fcca3513f1cc7fbf4d97d4b511f2378b3`): assign/unassign argument
      validation, assignee resolution, PUT request, and output now live in
      `cordy-cli/src/issue_assign_commands.rs`; parent dispatch retains its
      contract. The serial subagent reviewed the exact head and found no issue.
      Scoped rustfmt and `git diff --check` pass (the subagent environment
      lacked a rustfmt executable); Cargo was not run. Pro owns push/Ready PR,
      gate, and merge.
- [ ] CLI issue-reorder structural refactor is committed at
      `8e5cc1865d8cde9a25161fd65a286e1a40f6f4cf` (parent
      `d016ad65a8bfd880ce49113ec545cabd2ff7c276`, tree
      `a30b098ad9ff4afd031dce79417aa94f5f58bf35`): issue column loading,
      before/after/top/bottom validation, position calculation, target checks,
      and JSON/table output now live in
      `cordy-cli/src/issue_reorder_commands.rs`; parent position-calculation
      tests retain their `pub(super)` entry. The serial subagent reviewed the
      exact head and found no issue. Scoped rustfmt and `git diff --check`
      pass (the subagent environment lacked a rustfmt executable); Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-create structural refactor is committed at
      `f58670f7a049c55b355376c6f29088bd5bd357c6` (parent
      `8e5cc1865d8cde9a25161fd65a286e1a40f6f4cf`, tree
      `f9cc2a67fdf1ae706fb083ff34aa0e4d65cc8fad`), followed by the
      subagent's minimal compile fix `267c4136b24da35e9868af6a64b48c0c2f73b766`
      (tree `2069ec9d004e012a1e704bd9115f8a4cfcd0a094`): issue create
      validation, description/assignee/project/attachment resolution, POST and
      output now live in `cordy-cli/src/issue_create_commands.rs`; the fix
      imports `std::io::Read` for the extracted generic input. The serial
      subagent reviewed the resulting exact head and found no further issue.
      Scoped rustfmt and `git diff --check` pass (the subagent environment
      lacked a rustfmt executable); Cargo was not run. Pro owns push/Ready PR,
      gate, and merge.
- [ ] CLI issue-update structural refactor is committed at
      `ef653f06f0de0a5c18cfee0aa6bed4703b09ad18` (parent
      `267c4136b24da35e9868af6a64b48c0c2f73b766`, tree
      `6b4d51e973d58aaacbf96391f1b9f3f1555f0733`): issue update option
      validation, description/project/assignee/parent/stage/position handling,
      PUT request and output now live in
      `cordy-cli/src/issue_update_commands.rs`; parent shared helpers remain
      reusable. The serial subagent reviewed the exact head and found no issue.
      Scoped rustfmt and `git diff --check` pass (the subagent environment
      lacked a rustfmt executable); Cargo was not run. Pro owns push/Ready PR,
      gate, and merge.
- [ ] CLI issue pull-request command structural refactor is committed at
      `b1a8a1e7e63c9fc8fb3f1a36b8d5ed9618f86ccf` (parent
      `ef653f06f0de0a5c18cfee0aa6bed4703b09ad18`, tree
      `90aad55e91ab218fadf6d2ae8a1275a51deb2e31`): pull-request list/attach
      requests, payload serialization, validation, and table rendering now
      live in `cordy-cli/src/issue_pull_request_commands.rs`; parent dispatch
      and formatter tests retain their contracts. The serial subagent reviewed
      the exact head and found no issue. Scoped rustfmt and `git diff --check`
      pass (the subagent environment lacked a rustfmt executable); Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.

- [ ] CLI issue-children structural refactor is committed at
      `bf1aa6b5652948d3475dc397203672c425c4d830` (parent
      `b1a8a1e7e63c9fc8fb3f1a36b8d5ed9618f86ccf`, tree
      `ed49d44336ef39770ec67af8da37ff05c4ffc8eb`): child issue fetch,
      stage sorting/grouping, terminal counts, JSON envelope, and table
      rendering now live in `cordy-cli/src/issue_children_commands.rs`;
      parent dispatch and child formatter/grouping tests retain their
      contracts. The serial subagent reviewed the exact head and found no
      issue. Scoped rustfmt and `git diff --check` pass (the subagent
      environment lacked a rustfmt executable); Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI issue-list structural refactor is committed at
      `e6da5fe240c8e6469e4179830852edca24378a95` (parent
      `bf1aa6b5652948d3475dc397203672c425c4d830`, tree
      `539d6874638216ad51a187e5515542489f3c90eb`), followed by the
      subagent's minimal compile fix `cd8c20019a977eff2abec24b9f726b1fd36ecdfa`
      (tree `539d6874638216ad51a187e5515542489f3c90eb`): issue list
      query construction, metadata filter encoding, pagination envelope,
      and table/JSON orchestration now live in
      `cordy-cli/src/issue_list_commands.rs`; shared actor/assignee
      resolvers remain in the parent module. The serial subagent found and
      fixed the missing `BTreeMap` import, then rechecked the exact head
      with no further issue. Scoped rustfmt and `git diff --check` pass
      (the subagent environment lacked a rustfmt executable); Cargo was not
      run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-get structural refactor is committed at
      `a3eb2a893ce1d5334867faf5282f4ac445a1d3b6` (parent
      `cd8c20019a977eff2abec24b9f726b1fd36ecdfa`, tree
      `e24553587ed357301be988ff73525125f4c43efc`): issue fetch,
      actor enrichment, JSON output, and table formatting now live in
      `cordy-cli/src/issue_get_commands.rs`; shared issue-reference and
      actor helpers remain in the parent module. The serial subagent
      reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-actor output refactor is committed at
      `cac46a09ed4a107b0383a699411d97cac1544001` (parent
      `a3eb2a893ce1d5334867faf5282f4ac445a1d3b6`, tree
      `e082b6740f82a70149be15d727da49a40cc33a3e`), followed by the
      subagent's visibility fix `d226ef13626a496b35352f21aa0ca59d85c781aa`:
      actor-name loading, shared `IssueActorNames`, and issue-list table
      rendering now live in `cordy-cli/src/issue_actor_output.rs`; the
      tuple map field is explicitly visible to parent tests and sibling
      command modules. The serial subagent rechecked the exact head with no
      further issue. Scoped rustfmt and `git diff --check` pass (the
      subagent environment lacked a rustfmt executable); Cargo was not run.
      Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-actor resolver refactor is committed at
      `012b09471726ca94b9d003d8d4cae9b9b7898bf3` (parent
      `d226ef13626a496b35352f21aa0ca59d85c781aa`, tree
      `ae78ca6eb6b30868d06c6d23bc584580deff2592`): shared member/agent/
      squad lookup, assignee/subscriber resolution, project resolution,
      retry behavior, and typed `ResolvedIssueAssignee` now live in
      `cordy-cli/src/issue_actor_resolver.rs`; parent command imports
      retain their existing contracts. The serial subagent reviewed the
      exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-reference structural refactor is committed at
      `8975b4a8c4b2210ea13041889672d0f6d7f32ca6` (parent
      `012b09471726ca94b9d003d8d4cae9b9b7898bf3`, tree
      `137076e92dba9fa5f2eab9748d9edbc27042808a`): issue key/full-UUID
      recognition, short-prefix rejection, API resolution, and stable
      error messages now live in `cordy-cli/src/issue_reference.rs`.
      The serial subagent reviewed the exact head and found no issue.
      Scoped rustfmt and `git diff --check` pass (the subagent environment
      lacked a rustfmt executable); Cargo was not run. Pro owns push/Ready
      PR, gate, and merge.
- [ ] CLI task-run reference structural refactor is committed at
      `0c0e787f2d6aff5513c142a618707e3e999700f1` (parent
      `8975b4a8c4b2210ea13041889672d0f6d7f32ca6`, tree
      `09a02265511e8806f7233707d0c0a3a9aced04c9`): full UUID handling,
      issue-scoped short-prefix lookup, ambiguity/error reporting, and
      bounded task-run resolution now live in
      `cordy-cli/src/task_reference.rs`; parent and task command imports
      retain their contracts. The serial subagent reviewed the exact head
      and found no issue. Scoped rustfmt and `git diff --check` pass (the
      subagent environment lacked a rustfmt executable); Cargo was not run.
      Pro owns push/Ready PR, gate, and merge.
- [ ] CLI label-reference structural refactor is committed at
      `0ce120d412b21413d3eb3a3d7d968691713f138b` (parent
      `0c0e787f2d6aff5513c142a618707e3e999700f1`, tree
      `fc9865a26675c945d0fec71e9ab2b796f6fa96bf`): canonical UUID and
      prefix resolution, workspace URL encoding, ambiguity handling, and
      stable label errors now live in `cordy-cli/src/label_reference.rs`;
      parent and label command imports retain their contracts. The serial
      subagent reviewed the exact head and found no issue. Scoped rustfmt
      and `git diff --check` pass (the subagent environment lacked a
      rustfmt executable); Cargo was not run. Pro owns push/Ready PR, gate,
      and merge.
- [ ] CLI issue value/validation helper refactor is committed at
      `38efcc5d5f0733da5ae350c1831aa6b5a3b8065e` (parent
      `0ce120d412b21413d3eb3a3d7d968691713f138b`, tree
      `43dcfb43fd0a1c5a9b0efb22ac6c0c979e48c48b`): metadata formatting,
      issue-label extraction, and status/priority validation now live in
      `cordy-cli/src/issue_value_helpers.rs`; attachment staging and
      description helpers remain in the parent module. The serial subagent
      reviewed the exact head and found no issue. Scoped rustfmt and
      `git diff --check` pass (the subagent environment lacked a rustfmt
      executable); Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-description input refactor is committed at
      `c9f4de39116fbf5b3001feeb2faad87bd9d3261e` (parent
      `38efcc5d5f0733da5ae350c1831aa6b5a3b8065e`, tree
      `8aaa48f189cb854e414e801c92a5098b0294dd74`): inline/stdin/file
      source selection, workdir containment, newline trimming, and escape
      decoding now live in `cordy-cli/src/issue_description.rs`; create
      and update commands retain their existing contracts. The serial
      subagent reviewed the exact head and found no issue. Scoped rustfmt
      and `git diff --check` pass (the subagent environment lacked a
      rustfmt executable); Cargo was not run. Pro owns push/Ready PR, gate,
      and merge.
- [ ] CLI attachment-input refactor is committed at
      `397a741381ffe6d5cbaa7f8ce76137cad6b5dbbe` (parent
      `c9f4de39116fbf5b3001feeb2faad87bd9d3261e`, tree
      `74061f9cf6fb350d6a374812200864f05e45d290`): pending attachment
      staging type, attachment-ID deduplication, quick-create environment
      parsing, URL rejection, workdir containment, and local reads now live
      in `cordy-cli/src/attachment_input.rs`; create/comment imports retain
      their contracts. The serial subagent reviewed the exact head and found
      no issue. Scoped rustfmt and `git diff --check` pass (the subagent
      environment lacked a rustfmt executable); Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI issue-safety refactor is committed at
      `be099adcc275447cfe710f87e70d5c325b0e2034` (parent
      `9b60bfdef2a539345dc2dc4c91f07eeb77d9b68c`, tree
      `2fb5fc88fd7d31a3835c796a1d835a310dd535a2`): active-duplicate
      response decoding and runtime-local Markdown link detection now live
      in `cordy-cli/src/issue_safety.rs`; issue create/update/comment callers
      retain their existing contracts and parent tests keep the guard import.
      Scoped rustfmt and `git diff --check` pass; the serial subagent reviewed
      the exact head and found no compile or behavior blocker. Cargo was not
      run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI identifier-helper refactor is committed at
      `da3bd21660ab27e7a03b7333905067edad4c8793` (parent
      `be099adcc275447cfe710f87e70d5c325b0e2034`, tree
      `a8cc7ec1c616cb6600eebc863af14997aac2eb5a`): canonical UUID checks,
      normalized prefixes, and compact UUID matching now live in
      `cordy-cli/src/id_helpers.rs`; the parent re-exports the helpers so all
      existing command and test call-sites retain their contracts. Scoped
      rustfmt and `git diff --check` pass; the serial subagent reviewed the
      exact head and found no compile or behavior blocker. Cargo was not run.
      Pro owns push/Ready PR, gate, and merge.
- [ ] CLI output-helper refactor is committed at
      `25b649c21eac31bed413b78e90e73e2ea4d53804` (parent
      `da3bd21660ab27e7a03b7333905067edad4c8793`, tree
      `eab92ec92dfd1f33c85e6c180f89af0695bf5fa4`): table rendering, short
      identifier display, and bounded text truncation now live in
      `cordy-cli/src/output_helpers.rs`; the parent re-exports the helpers so
      existing command and test call-sites retain their contracts. Scoped
      rustfmt and `git diff --check` pass; the serial subagent reviewed the
      exact head and found no compile or behavior blocker. Cargo was not run.
      Pro owns push/Ready PR, gate, and merge.
- [ ] CLI text-input refactor is committed at
      `d13bdc667513d67e32a3f305c254648e0aee2e70` (parent
      `25b649c21eac31bed413b78e90e73e2ea4d53804`, tree
      `553e8a6b1e11d98a682d0f1f658e13f90cede91f`): one-newline trimming and
      backslash escape decoding now live in `cordy-cli/src/text_input.rs`;
      the parent re-exports them for issue/comment/user/workspace callers.
      Scoped rustfmt and `git diff --check` pass; the serial subagent reviewed
      the exact head and found no compile or behavior blocker. Cargo was not
      run. Pro owns push/Ready PR, gate, and merge.

- [ ] CLI URL-helper refactor is committed at
      `e3409bcc38a74f48739882df08c3c00c3b0e0f06` (parent
      `d13bdc667513d67e32a3f305c254648e0aee2e70`, tree
      `7c836b06ebde2d85b093e6877a78ca3743040c94`): path-segment encoding
      now lives in `cordy-cli/src/url_helpers.rs`; the parent re-exports it
      for skill, squad, and workspace MCP commands. Scoped rustfmt and
      `git diff --check` pass; the serial subagent reviewed the exact head and
      found no compile or behavior blocker. Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI API-client factory refactor is committed at
      `2d78cb93c28072d798f8931f730b3a3be0563eb5` (parent
      `e3409bcc38a74f48739882df08c3c00c3b0e0f06`, tree
      `04a655b9b720bc2f4cd09970abd3611ad6024f42`): client construction,
      task-context credential guards, server URL normalization, and workspace
      selection now live in `cordy-cli/src/client_factory.rs`; the parent
      re-exports the existing helpers. Scoped rustfmt and `git diff --check`
      pass; the serial subagent reviewed the exact head and found no compile
      or behavior blocker. Cargo was not run. Pro owns push/Ready PR, gate,
      and merge.
- [ ] CLI JSON value-helper refactor is committed at
      `1c50eb5f1b31bbbdbf064d0a31b5c3e9cc950394` (parent
      `2d78cb93c28072d798f8931f730b3a3be0563eb5`, tree
      `3daf62cec5cc11a292c215bef50597441a802b32`): null/string/JSON value
      conversion now lives in `cordy-cli/src/json_helpers.rs`; the parent
      re-exports it for all command and output helpers. Scoped rustfmt and
      `git diff --check` pass; the serial subagent reviewed the exact head and
      found no compile or behavior blocker. Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI agent-helper refactor is committed at
      `4c75e22391c680f1ede7c63dfea24ee27d1ee329` (parent
      `1c50eb5f1b31bbbdbf064d0a31b5c3e9cc950394`, tree
      `4ed192dd27727d75f128adde7a14bfd61616b809`): agent permission target
      construction, custom-env validation, secret JSON input, and agent table
      rendering now live in `cordy-cli/src/agent_helpers.rs`; the parent
      re-exports them for agent commands and tests. Scoped rustfmt and
      `git diff --check` pass; the serial subagent reviewed the exact head and
      found no compile or behavior blocker. Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI chat-output refactor is committed at
      `f84198080a5989d81556fbff2f86bd14784e3d1a` (parent
      `4c75e22391c680f1ede7c63dfea24ee27d1ee329`, tree
      `128ea7c74c40a14475fc5607a9a1086a23b7dd4e`): reply-count rendering now
      belongs to `chat_commands.rs`, removing the root-module dependency while
      preserving missing/zero/numeric JSON behavior. Scoped rustfmt and
      `git diff --check` pass; the serial subagent reviewed the exact head and
      found no compile or behavior blocker. Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI runtime-command refactor is committed at structural head
      `4c07bb8d0e33e5077f9fbf943b2f3dc53275e419`; its serial subagent found
      and minimally fixed a real moved-branch compile blocker in
      `5221ac56c4f8e5924d318e4f9ac087ef805230aa` (parent `4c07bb8d`, tree
      `752d08566a6b90c6769f4414acbb289c5523e3fb`): `run_runtime_*` list,
      usage, activity, rename, and delete now live in
      `cordy-cli/src/runtime_commands.rs`, and the delete error binding is
      corrected without changing behavior. `git diff --check` passes; Cargo
      was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI workspace-requirement refactor is committed at
      `1562c90672cc74f0491b2a87a6c81dff8f928f71` (parent
      `5221ac56c4f8e5924d318e4f9ac087ef805230aa`, tree
      `63a3a2db62798391481544c3144af0b56e14319a`): required workspace
      validation now belongs to `client_factory.rs`, preserving daemon
      context fail-closed behavior and all existing error text. Scoped
      rustfmt and `git diff --check` pass; the serial subagent reviewed the
      exact head and found no compile or behavior blocker. Cargo was not run.
      Pro owns push/Ready PR, gate, and merge.
- [ ] CLI version-output refactor is committed at
      `b1cc0061261ad0f6f7e460e0a0539310f76eeaaf` (parent
      `1562c90672cc74f0491b2a87a6c81dff8f928f71`, tree
      `008d3a233765343984a91710e13faa96a4436309`): text/JSON version output
      now lives in `cordy-cli/src/version_output.rs`, preserving all fields
      and newline formatting. Scoped rustfmt and `git diff --check` pass; the
      serial subagent reviewed the exact head and found no compile or behavior
      blocker. Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI autopilot list/get refactor is committed at
      `0d2471057d6ee287fa73a92ecc9bbce6a0b98f28` (parent
      `b1cc0061261ad0f6f7e460e0a0539310f76eeaaf`, tree
      `87a5c24e41955d0dd788b114c7350e95fe94e972`): read commands and their
      list envelope now live in `cordy-cli/src/autopilot_commands.rs`, while
      status URL encoding, workspace/agent resolution, and JSON/table output
      remain unchanged. Scoped rustfmt and `git diff --check` pass; the serial
      subagent reviewed the exact head and found no compile or behavior
      blocker. Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI autopilot mutation refactor is structurally committed at
      `a08c0a2ef27cf2c12dce3759fd89c518612372de`, with the serial subagent's
      visibility fix at `95574b89139ee21c6acc2fbce0880512450f8cc7` (parent
      `a08c0a2e`, tree `5fca7acab880a536ea5d4935048bbcc2d6d99baa`): create and
      update now live in `autopilot_commands.rs`; mode/title/agent/project/
      subscriber validation and output contracts are unchanged. Scoped
      rustfmt and `git diff --check` pass; Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI autopilot execution refactor is committed at structural head
      `29be90d5f77366971f2a2f92a45a347c857b14f9`; the serial subagent found
      and fixed the real visibility blocker in `9a1eb37cb48becfcdff635514ca6e22b58d3e94a`
      (tree `28dab750b418ed34ca47d2ec32b62979cad12e40`): delete, trigger, and
      runs execution now live in `autopilot_commands.rs`, with the parent
      re-exporting them and preserving timeout, query, resolution, and table/
      JSON behavior. `git diff --check` passes; rustfmt was unavailable in the
      subagent check and Cargo was not run. Pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI autopilot-trigger refactor is committed at structural head
      `159078c3a012ae96b4e76cc6976eba9e62631832`; the serial subagent found
      and fixed the real visibility blocker in
      `538441732ff6107798fc2ad3ba31d8bd83ace9c7` (tree
      `9346883b55776b51057ba9eac124feaa3291bdbc`): trigger add/update/delete,
      webhook rotation, and confirmation now live in `autopilot_commands.rs`,
      while URL generation, timeout, validation, and prompt behavior remain
      unchanged. `git diff --check` passes; rustfmt was unavailable in the
      subagent check and Cargo was not run. Pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI login-command refactor is committed at structural head
      `dc7d0c949d0a84511a41b19b83b97bd47bdeafe3`; the serial subagent fixed
      the missing serde import and child-module visibility in
      `e0c41139dab7505cc8ed1d49dba0d54bc0e07706` and
      `5c92dd516fcbacfbde21d44e6c98d40c46bc6b2a` (final tree
      `2ac8b4478f5525d69e8e0f65c8d6d817bed4ddd7`): login credential
      verification, browser fallback, workspace discovery, and atomic profile
      save now live with the existing login protocol module; behavior is
      unchanged. `git diff --check` passes; rustfmt was unavailable in the
      subagent check and Cargo was not run. Pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI daemon-outcome refactor is committed at
      `c2007c3ad6623612240fbd65722b927d8181b745` (tree
      `dcef4e17cf775cfc829e33a9f1a64abeac3b4778`): start/restart readiness,
      failure-evidence, and timeout rendering now belong to
      `daemon_commands.rs`; dispatch and all output/error semantics are
      unchanged. The serial subagent found no blocker; `git diff --check`
      passes, rustfmt was unavailable in the subagent check, and Cargo was
      not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI daemon-argument refactor is structurally committed at
      `7f6c3a2e015db84aaead1eeed0917e6b454eac3e`; the serial subagent fixed
      helper visibility in `36b1c12494c98fbfc05a460bdc1918dd3a94cf3f` (tree
      `ac37b7dc83e5871e341aa11ab1fd280ab36df95b`): restart foreground guard,
      profile-derived health-port validation, and Go-duration parsing now live
      in `daemon_commands.rs`, while clap parsers and error behavior remain
      unchanged. `git diff --check` passes; rustfmt was unavailable in the
      subagent check and Cargo was not run. Pro owns push/Ready PR, gate, and
      merge.
- [ ] CLI daemon-preparation-helper refactor is committed at
      `ba0a0d3a495886225af292de5019b45b34435353` (tree
      `e07c2052597726c38d3d2067cef394a3f758fb83`): the private execution
      environment protocol now lives in `daemon_commands.rs` and is publicly
      re-exported for `main.rs`; argv matching and stdin/stdout payload
      behavior are unchanged. The serial subagent found no blocker;
      `git diff --check` passes, rustfmt was unavailable in the subagent
      check, and Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI execution-policy refactor is structurally committed at
      `e8b364ccc30749fdb945c1f05ad8810edc343a85`; the serial subagent then
      fixed the real cross-module auth DTO visibility blocker in
      `38021d505b687219f9c08d90022a9e3f28d743b2` (tree
      `26fff6ef6c5c538e8cf1e6ed9c060380f460239a`): task-local configuration
      and human-only guards now live in `execution_policy.rs`, while
      login/auth share `AuthUser` through the parent module. `git diff --check`
      passes; rustfmt was unavailable in the subagent check and Cargo was not
      run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI command-output error refactor is committed at
      `57c6d7199369a4a033076b993714be12ad32789d` (tree
      `747bf1f8013c399f8c055c5dcb879a8acd173591`): the structured output error
      wrapper and public extractor now live in `error.rs`, preserving anyhow
      source-chain/downcast behavior for `main.rs`, skill commands, and tests.
      The serial subagent found no blocker; `git diff --check` passes, rustfmt
      was unavailable in the subagent check, and Cargo was not run. Pro owns
      push/Ready PR, gate, and merge.
- [ ] CLI command-dispatch refactor is committed at
      `dfb06c4b2bb4d5eb130bba7633730b093cbd2fc1` (tree
      `5c6890759a204a8e0640549faf75ba92a0b569ba`): the complete
      `run_with_input` command match now lives in `command_dispatch.rs`, with
      the root `run` and tests using the parent re-export; every branch,
      stdin stream, and output contract is unchanged. The serial subagent
      found no blocker; `git diff --check` passes, rustfmt was unavailable in
      the subagent check, and Cargo was not run. Pro owns push/Ready PR, gate,
      and merge.
- [ ] CLI daemon-launch mapping refactor is committed at
      `0cc4a1c314b0ce1655cc740d58adff727bf634f4` (tree
      `b5f8adb7e93344727a41daae5184b297031619e8`): the complete
      `DaemonLaunchArgs::to_launch_flags` mapping now belongs to
      `daemon_commands.rs`; all timeout, concurrency, auto-update, and reload
      fields remain identical. The serial subagent found no blocker;
      `git diff --check` passes, rustfmt was unavailable in the subagent
      check, and Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI debug-policy refactor is committed at
      `c43939b370142a0586d20a4c53418195e7a5fe70` (tree
      `43042c5e9631386e44ab47c3f54960500ca7d1e6`): `Cli::debug_enabled` now
      lives with error presentation in `error.rs`, while `main.rs` keeps the
      same public method and `CORDY_DEBUG` truth table. The serial subagent
      found no blocker; `git diff --check` passes, rustfmt was unavailable in
      the subagent check, and Cargo was not run. Pro owns push/Ready PR, gate,
      and merge.
- [ ] CLI autopilot-schema refactor is structurally committed at
      `0ba9843554324c2491ebb91acf62e371ea2aef29`; the serial subagent fixed
      schema type/field visibility in `fb1342513d6d7cc312d4722cda52173fb2d3cae8`
      (tree `88adda9b0919528eedd1e3a1932daffd7e16c2bd`): autopilot command,
      trigger, create, and update clap definitions now live beside their
      handlers, with defaults and parser metadata unchanged. `git diff --check`
      passes; rustfmt was unavailable in the subagent check and Cargo was not
      run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI runtime-schema refactor is structurally committed at
      `edebfa932d9d3fbd4c32e0d4c7547444b1dd778c`; the serial subagent fixed
      schema type/field visibility in `ff568e5b5158c11cffc6406dbe46a7c4f47d2a10`
      (tree `0c0a5a03c445b37c52cc34fd16c2aba6556c3818`): runtime and custom
      profile clap definitions now live beside runtime handlers, with dispatch,
      profile commands, defaults, and parser metadata unchanged. `git diff
      --check` passes; rustfmt was unavailable in the subagent check and Cargo
      was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI agent-schema refactor is structurally committed at
      `7b87fbacfdf86dadaa6f231a46cd7d254e57e731`; the serial subagent fixed
      schema type/field visibility in `ceda4412aceae27df9be700b67e2da810f6a095e`
      (tree `6254036df8a32ceede586808348cc30a6d1f3185`): agent, MCP, env,
      skills, copy, create, and update clap definitions now live beside agent
      handlers, with parser metadata and behavior unchanged. `git diff
      --check` passes; rustfmt was unavailable in the subagent check and Cargo
      was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI repository-schema refactor is structurally committed at
      `d2db878ff28f94871308f1e621794e97bb447c38`; the serial subagent fixed
      schema type/field visibility in `0fa4203ea1ece6f6711ed588dbdca97c71cd9abb`
      (tree `0957e550238fd4b4d4d28346747e93170f1744ae`): repo list/add/remove/
      checkout clap definitions now live beside repository handlers, with
      aliases, positional URL behavior, and output defaults unchanged. `git
      diff --check` passes; rustfmt was unavailable in the subagent check and
      Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI chat/attachment-schema refactor is structurally committed at
      `81faa66e1cda0a0f87ab23e5cdc15429cb024b9b`; the serial subagent fixed
      schema type/field visibility in `b42f4dd3c79d3de89ee0aa96aecd89f7d3ac9064`
      (tree `99742de6eb48fecbfd32fc97867a7654925d72df`): attachment download/
      upload and chat history/thread clap definitions now live beside their
      handlers, with path semantics and output defaults unchanged. `git
      diff --check` passes; rustfmt was unavailable in the subagent check and
      Cargo was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI property-schema refactor is structurally committed at
      `201c5c4cc4daf8ba6dfde147f692636dcab341ba`; the serial subagent fixed
      schema type/field visibility in `3b70ce157c147ab68981383530be2c4dcf566b44`
      (tree `7a923e9a58355b0131a76b582498ac1fd842589b`): property list/get/
      create/update/archive/unarchive clap definitions now live beside property
      handlers, with type/options/default metadata unchanged. `git diff
      --check` passes; rustfmt was unavailable in the subagent check and Cargo
      was not run. Pro owns push/Ready PR, gate, and merge.
- [ ] CLI issue-schema refactor is structurally committed at
      `cd0c86b871e2017e2dfff8830f8e194f20798d95` (tree
      `dbdc29eedde1b6a538f74f3cf6bb373f8831f744`): the `IssueArgs` and
      `IssueCommand` clap schema now live in `issue_command_schema.rs`, while
      all issue argument payloads, dispatch patterns, aliases, and output
      defaults remain unchanged. Scoped rustfmt with `skip_children` and
      `git diff --check` pass; Cargo was not run. The serial subagent owns
      blocker-only fixes/review; Pro owns the eventual Ready PR, gate, and
      merge.
- [ ] CLI label-schema refactor is structurally committed at
      `9e5851802337b1d9134d9848b5349fa3e995e180` (tree
      `8442fb1d7d3a4db49113caa4444e3694337d4474`): label list/get/create/
      update/delete clap definitions now live in `label_command_schema.rs`;
      command aliases, required values, and output defaults are unchanged.
      Scoped rustfmt with `skip_children` and `git diff --check` pass; Cargo
      was not run. The serial subagent owns blocker-only fixes/review; Pro
      owns the eventual Ready PR, gate, and merge.
- [ ] CLI project-schema refactor is structurally committed at
      `957beac6ef2527794064af8a59f64056d0ffe829` (tree
      `871a8f98c66cc98529377b834c230360c961a339`): project and
      project-resource clap schemas now live in `project_command_schema.rs`,
      with resource shortcuts, defaults, aliases, and output behavior
      unchanged. Scoped rustfmt with `skip_children` and `git diff --check`
      pass; Cargo was not run. The serial subagent owns blocker-only
      fixes/review; Pro owns the eventual Ready PR, gate, and merge.
- [ ] CLI issue-core schema refactor is structurally committed at
      `c867dab48db1e4991c92e70a37c5acfc9061d47b` (tree
      `2de49399022979a4c46f7d94001a780262f8e44a`): create/update/assign/
      status/reorder argument schemas now live beside `IssueCommand` in
      `issue_command_schema.rs`; clap groups, help text, defaults, and field
      semantics remain unchanged. Scoped rustfmt with `skip_children` and
      `git diff --check` pass; Cargo was not run. The serial subagent owns
      blocker-only fixes/review; Pro owns the eventual Ready PR, gate, and
      merge.
- [ ] The issue-core stack's real visibility blocker was fixed by the serial
      subagent in `b961d619f7af628215e2f62f2bcdbe57b0f1cccb` (tree
      `63b1fe763e54c09bab8cd214da1fdfe5318daa73`): the parent re-exports all
      moved create/update/assign/status/reorder schemas needed by sibling
      handlers. No behavior or production logic changed; Cargo was not run.
- [ ] CLI issue-activity schema refactor is structurally committed at
      `205f8d4de7b8d77a78d5292011a71f8c8d516fe1` (tree
      `62939ff0e2729e084c7e70911d1a3ba06386e7cb`): comment, execution-history,
      rerun/cancel, usage, and search clap schemas now live in
      `issue_activity_schema.rs`; subcommand aliases, help text, defaults, and
      field semantics remain unchanged. Scoped rustfmt with `skip_children`
      and `git diff --check` pass; Cargo was not run. The serial subagent owns
      blocker-only fixes/review; Pro owns the eventual Ready PR, gate, and
      merge.
- [ ] CLI issue-subscriber schema refactor is structurally committed at
      `e19c268708e5cf81d538b0ec6c0b39b9e0fb1453` (tree
      `dce3c97d9beabef272f567a0e11f1b13170967df`): subscriber list/add/remove
      clap definitions now live in `issue_subscriber_schema.rs`; caller
      defaults, mutually-exclusive identity flags, and output behavior remain
      unchanged. Scoped rustfmt with `skip_children` and `git diff --check`
      pass; Cargo was not run. The serial subagent owns blocker-only
      fixes/review; Pro owns the eventual Ready PR, gate, and merge.
- [ ] CLI issue-metadata schema refactor is structurally committed at
      `6183b4c78b6da44d57cf46295f49ab6aad3df1df` (tree
      `dff106b155c62c6040b5302d797f2aba7629f236`): metadata list/get/set/delete
      clap definitions now live in `issue_metadata_schema.rs`; key/value/type
      flags and output defaults remain unchanged. Scoped rustfmt with
      `skip_children` and `git diff --check` pass; Cargo was not run. The
      serial subagent owns blocker-only fixes/review; Pro owns the eventual
      Ready PR, gate, and merge.
- [ ] CLI issue-property schema refactor is structurally committed at
      `152fec3b17b2a3d89550ca98629157da89cd2f82` (tree
      `506cb2c1b52b8576f8715bc326c71ec821482f0d`): issue property list/set/
      unset clap definitions now live in `issue_property_schema.rs`; names,
      values, type-independent parsing, and output defaults remain unchanged.
      Scoped rustfmt with `skip_children` and `git diff --check` pass; Cargo
      was not run. The serial subagent owns blocker-only fixes/review; Pro
      owns the eventual Ready PR, gate, and merge.
- [ ] CLI issue-timeline schema refactor is structurally committed at
      `b0709fcd53398f55aa0e15d6d320409e13e0a892` (tree
      `f3407a7bee214b67410dcb6221a88ff3008b2a96`): timeline filters,
      pagination, action selection, and output flags now live in
      `issue_timeline_schema.rs`; defaults and query semantics remain
      unchanged. Scoped rustfmt with `skip_children` and `git diff --check`
      pass; Cargo was not run. The serial subagent owns blocker-only
      fixes/review; Pro owns the eventual Ready PR, gate, and merge.
- [ ] CLI issue pull-request schema refactor is structurally committed at
      `bd18427f1b76637ba3380ba622c3e4b7c2afcf11` (tree
      `054ad92f044d403d12c00b597a788b54c3610769`): pull-request attach
      subcommand and URL/title/state/branch/SHA parameters now live in
      `issue_pull_request_schema.rs`; validation and table/JSON defaults are
      unchanged. Scoped rustfmt with `skip_children` and `git diff --check`
      pass; Cargo was not run. The serial subagent owns blocker-only
      fixes/review; Pro owns the eventual Ready PR, gate, and merge.
- [ ] CLI issue-list schema refactor is structurally committed at
      `11eac434116e433fb4d20ba325d968a0b6d540f7` (tree
      `dfd0d91719c5097ee80bcacaea773e4797cb5fab`): issue filters, metadata
      selectors, pagination, sort, and direction flags now live in
      `issue_list_schema.rs`; defaults and query semantics remain unchanged.
      Scoped rustfmt with `skip_children` and `git diff --check` pass; Cargo
      was not run. The serial subagent owns blocker-only fixes/review; Pro
      owns the eventual Ready PR, gate, and merge.
- [ ] CLI issue-label schema refactor is structurally committed at
      `476357eca0dfede91b4a00aef9db82815fe5e117` (tree
      `ff41f7b159599379ce95eed8c52c2f85a5c9b006`): issue label list/add/remove
      clap definitions now live in `issue_label_schema.rs`; label IDs,
      full-ID output, and defaults remain unchanged. Scoped rustfmt with
      `skip_children` and `git diff --check` pass; Cargo was not run. The
      serial subagent owns blocker-only fixes/review; Pro owns the eventual
      Ready PR, gate, and merge.
- [ ] CLI daemon-schema refactor is structurally committed at
      `18d4e6f36930da8d5205712197abdb01439fd3ee` (tree
      `b71340049988815c780a3b7fb3a8b6902f9e1554`): daemon subcommands,
      start/restart launch flags, logs, status, and disk-usage argument schema
      now live in `daemon_command_schema.rs`; lifecycle dispatch and all flag
      defaults remain unchanged. Scoped rustfmt with `skip_children` and
      `git diff --check` pass; Cargo was not run. The serial subagent owns
      blocker-only fixes/review; Pro owns the eventual Ready PR, gate, and
      merge.

- [ ] CLI config-schema refactor is structurally committed at
      `57eae832ddd21222ff47077b935f5619a7cf0eb8` (tree
      `8e8355cb6c1ea775cc4d5a0cc7d1d4a37a94ee09`): config show/set clap
      definitions now live in `config_command_schema.rs`; optional command
      dispatch, keys, values, and output defaults remain unchanged. This
      extends Ready PR #159; scoped rustfmt with `skip_children` and
      `git diff --check` pass, Cargo was not run.
- [ ] CLI auth/login schema refactor is structurally committed at
      `e8ad789bb0960f652de7b3a43b4eaa5bb9b70768` (tree
      `ff977195e963e6831c3f8a18ae378e917a1afbdf`): auth status/logout and
      login token/callback flags now live in `auth_command_schema.rs`; auth
      dispatch and browser-login semantics remain unchanged. This extends
      Ready PR #159; scoped rustfmt with `skip_children` and `git diff --check`
      pass, Cargo was not run.
- [ ] CLI user/profile schema refactor is structurally committed at
      `b2301f0263b49ded70404342bfb7f9a59e273d75` (tree
      `a6533ad745edac2465ea2e13a78c7df58e7a8253`): user/profile command,
      profile update input, path, clear, and output flags now live in
      `user_command_schema.rs`; profile update behavior remains unchanged.
      This extends Ready PR #159; scoped rustfmt with `skip_children` and
      `git diff --check` pass, Cargo was not run.

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
