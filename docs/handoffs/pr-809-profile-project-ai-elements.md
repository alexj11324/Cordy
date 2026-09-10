# PR #809 handoff: Projects, Profile, and AI Elements

## State at pause

- Pull request: <https://github.com/alexj11324/Cordy/pull/809>
- Branch: `codex/profile-project-ai-elements`
- Canonical worktree: `/Users/alexjiang/.cache/cordy-profile-project-ai-20260910`
- Base: merged `main` at `15165505a209f3aae173a4bdc7ae222ab928f5e7`
- The pull request is intentionally Draft. Do not merge until a new exact-head CI run is green.
- The primary checkout at `/Users/alexjiang/Desktop/vibe/Cordy` contains unrelated user changes in `packages/views/projects/components/projects-page.tsx`, `.omo/`, and `vendor/`. Leave them untouched.

## Implemented scope

The branch combines the remaining project workflow, Profile, review handoff, and conversation work after PRs #806 and #807:

- project metadata, summaries, workspace leads, seed fixtures, and ReUI Projects DataGrid;
- complete Profile details and the scoped email-change verification flow;
- issue Agent popover cleanup, full property values, and removal of the obsolete Stats entry;
- all 18 AI Elements categories in real shared Web/Desktop surfaces, plus native Mobile equivalents;
- provider-to-UI tool `call_id` and state, task usage, attachments, sources, UTF-8 citation ranges, confirmation, plans, and checkpoints;
- attachment-only sends and attachment-id-aware idempotency;
- execution review handoff and discovery retry behavior.

Database migrations 611 through 614 belong to this branch.

## Final review fixes in the pause commit

The last review identified three concrete blockers. All three are fixed locally and the same reviewer returned APPROVE:

1. Web citation offsets now pass through a shared Markdown-aware insertion helper before issue autolinking, raw URL linkification, and file-card preprocessing.
2. Mobile uses the same helper, so citation markers cannot split inline code, authored links, images, or other atomic Markdown nodes.
3. Live and persisted assistant output use the same `AssistantRichContent` component, preserving Mermaid instances across the handoff.

The commit also fixes every failure reported by the previous PR CI head:

- replaces forbidden Tailwind font sizes and transparent text colors in imported AI Elements/ReUI components;
- localizes MessageUsage labels and accessibility text;
- updates the Agent entrypoint contract for the intentional single conversation entry;
- updates the issue surface batch mock to the current `{ updated }` contract;
- updates the Settings test to the current “Authorized clients” label.

## Verified evidence

Passed after the final review fixes:

- Core, Views, and Mobile typechecks;
- Core Markdown marker tests: 4/4;
- focused Views citation and Mermaid tests: 39/39;
- focused CI regression group: 108/108;
- Mobile attribution tests: 5/5;
- full Mobile Vitest: 45 files, 228 tests;
- Web text contrast and type scale: 34/34;
- UI and Views ESLint: zero errors;
- `git diff --check`;
- independent code review: APPROVE, zero blocking findings.

Earlier branch evidence remains valid for the unchanged paths:

- fresh isolated PostgreSQL migrations through 614;
- focused Go service, daemon, handler, middleware, migration, protocol, and server suites;
- Electron runtime checks for the DEV-14 Agent popover, full property values, Projects grid, Profile form, and Change email entry;
- screenshots at `~/.cache/codex-tmp-10g/cordy-agent-popover.png`, `cordy-projects-reui.png`, and `cordy-profile3.png`.

## Remaining work

The user paused work while the exact CI build command was running, so it was stopped. The previous remote head `ee5fdbc6` had red frontend jobs; their reported failures are fixed in the pause commit, but a new CI run has not yet verified them.

Resume in this order:

1. Inspect PR #809 and confirm the remote head matches this pause commit.
2. Wait for the new exact-head CI run. Use the current head SHA when judging checks; ignore the superseded `ee5fdbc6` run.
3. Fix only reproducible failures on the current head. Local whole-suite runs under Node 26 produced missing-`localStorage` failures; use Node `22.23.2`, which is the repository and CI runtime.
4. Read all unresolved GitHub review threads, fix valid findings, reply, and resolve them.
5. Re-run the affected real Electron paths if a UI fix changes behavior.
6. Mark the pull request ready only after exact-head CI and review are clean. Merge only after the user resumes and authorizes continued delivery.

Useful commands:

```bash
cd /Users/alexjiang/.cache/cordy-profile-project-ai-20260910
eval "$(fnm env)"
fnm use 22.23.2
gh pr view 809 --json headRefOid,isDraft,statusCheckRollup,reviewDecision
pnpm exec turbo build typecheck lint --filter='!@orvilo/mobile'
pnpm exec turbo test --filter='!@orvilo/docs' --filter='!@orvilo/mobile' --filter='!@orvilo/views'
pnpm --filter @orvilo/views test --shard=1/2
pnpm --filter @orvilo/views test --shard=2/2
```

Do not continue polling, fixing, replying, pushing, marking ready, or merging while this pause remains in effect.
