# PR #777 handoff — Cursor-style automation Settings (Codex)

**Audience:** Codex taking over this branch. Read this file first; the PR body used to describe an older layout and is no longer authoritative.

| | |
|---|---|
| Repo | `https://github.com/alexj11324/Cordy` (fork; some tools need lowercase `cordy`) |
| Branch | `cursor/cloud-agent-1788649529004-9qees` (based on `main`) |
| PR | https://github.com/alexj11324/cordy/pull/777 (draft) |
| Human language | Reply to the user in **简体中文**. Code comments stay English. |
| Visual target | Cursor Automations Settings screenshot (user-attached). Local copy on the previous Cloud Agent VM: `/home/ubuntu/.cursor/projects/workspace/assets/4F1E940D-53E2-4B0D-87BF-09E388353FBD_L0_001.jpg` — re-open the user attachment if this path is missing. |
| Related | #769 is **already merged**. Do not amend it. Do not change backend fan-out / OAuth / webhook implementation. Do not restyle the automations **list** page. |

## Why this PR exists

Match the shared `packages/views` automation **Settings detail page** to Cursor Automations chrome **and** keep every control a real write/navigation, not a dead shell.

Autosave already exists (debounced). Do **not** invent a fake Save. Product copy stays Patchbay: **Run now** (not Test), **No project** (not No Repository), keep **Edit**. Do **not** add Microsoft Teams / Sentry / PagerDuty as trigger sources.

## Status at handoff

Layout + mutation wiring are in code and covered by Vitest. **Electron click → real DB/API proof is not done.** That is the next job.

Commits on this branch (oldest → newest):

1. `ae8d6df79` `feat(automations): match Cursor settings layout on the shared detail page`
2. `72ac87c3f` `fix(automations): match Cursor settings chrome against the product screenshot`
3. `56168f346` `fix(automations): make the daily time chip and metadata rules visible`
4. `9f2e49d75` `fix(automations): prove settings controls write, not just render`
5. (this file) `docs(automations): Codex handoff for Cursor-style settings`

CI was green on `72ac87c3f` and `56168f346` (20/20). Recheck the latest SHA after you push.

## What Codex should do next (in order)

1. **Prove writes on Electron, not just mock mutations.** Click the real Settings page, then `GET /api/automations/:id` (or SQL) and show cron / status / triggers changed. Toast-only is a fail.
   - Daily time chip `07:00` → `08:00` → `cron_expression` contains `0 8 * * *` (keep any `TZ=` prefix).
   - `+ Add Trigger` → GitHub → **Draft opened** → new trigger with preset `github.draft.opened`.
   - Inactive switch on → `status=active`. Then **Run now** becomes enabled (disabled while paused is a **product gate**, not a dead button).
   - **Connect** on an unconnected GitHub trigger navigates to workspace settings `?tab=github`.
   - Memories Manage switch persists `tools.memories.enabled`.
2. **Keep visual parity** with the screenshot while doing that: capsule tabs under the title, one Triggers card, gray time chip, Add Trigger search + GitHub Only, no outer “Agent Instructions” heading, white editor, compact borderless model chip, Tools as loose rows. Switching to Run History must **keep** the large title + meta row + capsule tabs.
3. If a control is a shell, wire it. If Electron HMR whitescreens the detail page, hard-refresh before treating it as a product bug (`GitBranch` / `sourceIcon` / `cn` is not defined happened mid-HMR).
4. After visual or layout tweaks, extend the existing Vitest files rather than inventing a second suite.
5. Do not edit `/opt/cursor/artifacts/plans/`.

Success looks like: GET proof of writes + screenshot/video still matching the Cursor chrome, with Patchbay product nouns.

## Hard constraints

- UI lives in `packages/views/` only. `apps/web` and `apps/desktop` automation pages **re-export** the shared components; do not fork the page.
- Font sizes: only the role-named `--text-*` scale in `packages/ui/styles/tokens.css`. No `text-sm` / `text-base` / `text-[Npx]`.
- Chinese copy: `apps/docs/content/docs/developers/conventions.zh.mdx`.
- No extra local state unless the design needs it. Prefer existing shadcn/Base UI.
- Connect links that wrap `AppLink` **must** set `nativeButton={false}` on `Button`. Omitting it produces a Base UI warning and the `<a>` may not act like a real link.
- Do not bring back:
  - an outer “Agent Instructions” page heading
  - `TabsList variant="line"` as the Settings/Run History chrome (current: transparent list, `data-active:bg-muted`, `after:hidden`)
  - Cursor nouns (Test / No Repository / IDE Save)

## Target chrome (screenshot)

```
breadcrumb: Automations > {name}
large title (text-display-sm, bold)
[Inactive switch] | [No project picker, no border] | By {author}
[Settings] [Run History]     ← capsules under the title, selected = bg-muted, no underline track

TRIGGERS                     ← small muted uppercase
┌ card ─────────────────────────────────────────┐
│ clock  Every day at  [07:00 ▾]  EDT  Next run…│
│ github … Requires connection        [Connect] │
│ + Add Trigger                                 │
└───────────────────────────────────────────────┘
auth warning (settings.triggers_auth_warning)

## markdown is the instructions title (no outer Agent Instructions heading)
white editor, compact ModelDropdown bottom-left (no Cpu icon, no border)

TOOLS                        ← loose rows, no outer card
Memories
Send to Slack
+ Add MCP
```

Header actions stay **Edit**, **Run now**, `…` delete. Cursor’s gray Save / Test are not our product.

## Wiring map (already in code)

| Control | Write / navigation |
|---|---|
| Inactive / Active switch | `updateAutomation({ status: "active" \| "paused" })` |
| Project picker | `updateAutomation({ project_id })` |
| Daily time chip | `updateTrigger({ cron_expression: toCron(...) })`, preserves timezone via cron `TZ=` prefix |
| Trigger enable / delete | `updateTrigger({ enabled })` / `deleteTrigger` — **opacity-0 until `group-hover/trigger` or `focus-within`**. computerUse often cannot see them; they are not dead. |
| Add Trigger → GitHub event | `createTrigger({ kind: "webhook", preset: preset.id })`. Backend infers provider from preset. |
| Add Trigger → Scheduled | Opens existing `AddTriggerDialog` / schedule editor; submit is a real schedule create. |
| Connect (GitHub) | `settingsPathForTriggerProvider` → `?tab=github` |
| Connect (Slack / Linear) | `?tab=integrations` |
| Memories | `updateAutomation({ tools: { memories: { enabled } } })` |
| Slack send / MCP add | same tools persist helper in `automation-tools-section.tsx` |
| Model chip | `updateAutomation({ model })` |
| Instructions editor | existing autosave (do not add Save) |
| Run now | `useTriggerAutomation`; **disabled when `status !== "active"`** |

## Key files

```
packages/views/automations/components/automation-detail-page.tsx
packages/views/automations/components/automation-detail-page.test.tsx
packages/views/automations/components/trigger-card.tsx
packages/views/automations/components/trigger-add-menu.tsx
packages/views/automations/components/trigger-add-menu.test.tsx
packages/views/automations/components/automation-tools-section.tsx
packages/views/agents/components/model-dropdown.tsx          # compact: no Cpu, no border
packages/views/editor/styles/prose.css                      # .automation-instructions h2 = --text-title-sm
packages/views/locales/{en,zh-Hans,ja,ko}/automations.json
packages/core/automations/                                  # catalog, settingsPathForTriggerProvider
```

i18n notes:

- `settings.schedule_every_day_at` is **no longer** interpolated with the time; the time lives in the chip.
- `settings.schedule_time_aria` labels the Select.
- `settings.tools_add` = “Add MCP”.
- EN GitHub preset labels were moved toward the screenshot (Draft opened / Pull request opened / GitHub Only).

## Tests already asserting writes

`automation-detail-page.test.tsx`:

- Capsule tabs under the title (`data-active:bg-muted`, `after:hidden`).
- Add Trigger lives **inside** the Triggers card.
- No “Agent Instructions” heading; compact model chip inside the editor.
- Tools rows: Memories / Send to Slack / Add MCP + auth warning copy.
- **Real mutation wiring:** Connect hrefs, activate switch → `status: "active"`, time `08:00` → cron `0 8 * * *`, Add Trigger → `github.draft.opened`, Memories persist.

`trigger-add-menu.test.tsx`: open GitHub submenu; pick Draft opened → `onPickPreset`.

```bash
pnpm --filter @patchbay/views exec vitest run \
  automations/components/automation-detail-page.test.tsx \
  automations/components/trigger-add-menu.test.tsx
pnpm --filter @patchbay/views exec tsc --noEmit
```

These tests mock `@patchbay/core` mutations. They do **not** prove Electron wrote the database.

## Local environment notes (previous Cloud Agent VM)

Treat as hints; re-read `make status` on a new machine.

- Environment name was `workspace-429`.
- `make up C=api,desktop` (Web often cannot start here: missing Clerk key). Prefer Electron.
- `make dev-login` — `PATCHBAY_DEV_LOGIN=1` is written by `make up`. **Do not** request a login verification code.
- API was `:18509`, desktop renderer `:5603`, `DISPLAY=:1`.
- DB `patchbay_workspace_429`, user `dev@localhost`, workspace slug `dev`, id `ae94e97e-8bad-4f44-ba54-c8ae01853553`.
- Fixture automation `4515b386-1cb7-4b55-b2bc-9446f37b0086` titled **Find critical bugs**.
- Last SQL snapshot before the unfinished Electron pass: `status=paused`, schedule `cron_expression: "0 7 * * *"` tz `America/New_York`, GitHub trigger `github.pull_request.opened`.
- Scout agent may have been inserted via SQL; list can show “Unknown Agent”; the detail page still loads.
- Slack installations poll every 5s — noisy desktop logs, ignore.
- Instructions were PATCH’d locally to `## Investigation strategy` + bullets for screenshot parity. That is **local DB only**, not git.

Acceptance artifacts belong under `/opt/cursor/artifacts/` with **new filenames**. Do not reuse a discarded screen recording from the previous run. If you record a video, run the `videoReview` subagent before citing it.

## Pitfalls

- Schedule row enable/delete are `opacity-0` until hover/focus-within. Do not call them dead because a bot cannot hover.
- Time chip gray fill is an outer `bg-muted` wrapper around `Select`. `SelectTrigger` is `bg-transparent`; putting the muted class only on the trigger gets eaten.
- Meta separators are `h-3.5 w-px bg-border` — very faint; they are intentional.
- `gh` in this environment is read-only. Open/update PRs with the Cursor `ManagePullRequest` tool. Use `https://github.com/alexj11324/cordy/pull/777` (lowercase) if the tool rejects `Cordy`.
- Do not persist server data in Zustand. React Query owns automations.

## Out of scope

- Automations list page restyle
- Backend webhook fan-out, GitHub App OAuth, secret storage
- Teams / Sentry / PagerDuty catalog entries
- Fake Save button
- Mobile
- Reopening or rewriting #769
