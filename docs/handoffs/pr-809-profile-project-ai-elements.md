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

## Resumed delivery update (2026-09-10)

The user has resumed delivery. The current worktree adds the review fixes that were pending at the pause:

- Mobile existing-issue and new-issue review transitions collect and submit worktree, branch, full commit SHA, and PR evidence atomically with status and reviewer.
- Agent thread events preserve structured sources and UTF-8 citation ranges, and the selected Agent thread renders in the global right sidebar.
- Same-Agent independent conversation roots remain selectable, non-PR work products stay visible, and summary-only project search results include the matching snippet.
- Profile description and locale persistence, builder modal reachability, and lazy builder-session cleanup are covered by focused tests.

Current local evidence after these additions: Mobile Vitest 45 files / 231 tests plus iOS script assertions, Core Vitest 175 files / 1,968 tests, focused Views tests 8 files / 51 tests, Core / Views / Mobile typechecks, Mobile and Views lint with zero errors, Go project-summary handler tests, and `git diff --check`.

## Remaining work

Commit and push the verified worktree changes, wait for a new exact-head CI run, fix only reproducible failures on that head, reply to and resolve every valid GitHub review thread, mark the PR ready, and merge after all required checks pass.

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
