# PR #777: automation trigger and interaction audit

This handoff records the current PR source and the evidence gathered for trigger
readiness, provider conditions, the shared Electron settings surface, and the
Agent continuation panel. The PR source merge and conflict resolution were
complete at head `530d99b6` before this documentation update. The local primary
checkout was restored to clean `main` at `cdc795a3` during recovery; `main` may
advance after that recovery point.

The live runtime evidence below is labeled separately: Electron, API, and the
daemon were last started from `ce0d1248` with Go 1.26.7. That runtime was not
rebuilt after the final source merge to `530d99b6`, so it is evidence of the
merged implementation before the documentation-only commit, not final-head
runtime acceptance.

## Trigger semantics

A trigger starts the automation's assigned agent or team with the configured
instructions and event payload. The execution mode decides whether to run a
task directly (`run_only`) or create an issue (`create_issue`). The project
supplies execution context. A GitHub repository filter controls which
repository's events may start an automation; the execution project alone does
not filter event sources.

Trigger readiness is server-owned admission. An automation with zero triggers,
an unconnected provider, a missing required parameter, or a persisted legacy
disabled trigger cannot run. The read path reports the reason without rewriting
the trigger. For Slack `channel_created`, an omitted `installation_id` keeps
the Any-workspace wildcard semantics and is ready when the workspace has an
installed, nonpaused Slack connection. Provider lookup failures propagate as
errors. Durable webhook deliveries stay queued and retry instead of becoming
terminal skipped runs.

| Source | Event choices inspected in Cursor | Cordy implementation |
| --- | --- | --- |
| Schedule | Scheduled runs | Full time, interval, weekly/monthly, timezone, and advanced cron editing for existing and new schedules |
| GitHub | Draft opened; PR opened/pushed/merged; comments; branch push; labels; CI; issue/review comments; submitted reviews; review threads; workflow completion | Event-specific presets and conditions, including PR authors, pushed branch, changed label, review result, thread state, and workflow conclusion |
| Slack | Channel messages, reactions, channel creation | Connected rows expose installation/channel conditions; `channel_created` supports Any connected workspace; message/reaction conditions retain keyword or regex, authenticated-sender, thread-reply, and completion-reaction controls |
| Linear | Issue created, issue status changed, cycle ended | Real team/project/status IDs from the connected provider, matched against webhook fields |
| Webhook | Webhook triggered | Generated URL, copy, rotation, delivery, and replay paths |

Cursor's GitHub submenus explicitly offer review results (approved, changes
requested, commented, any), thread states (resolved, unresolved, any), and
workflow conclusions (success, failure, cancelled, any). Cordy exposes these on
the corresponding trigger. Cursor's PR-opened row contains repositories and
author without an extra branch/label panel; the extra panel was removed.
Cursor's branch-push row contains repository, branch, and author. New Slack
message triggers default to ignoring thread replies while existing saved
configurations preserve their meaning. CI uses check-suite completion rather
than firing independently for each check.

## Readiness and manual-run behavior

Run Now and every dispatch path use the same readiness gate. In the final
Electron interaction, clicking Run Now showed the localized modal title
`请先完成触发器配置`, its explanatory description, and the `返回配置` action.
The modal exposed no top-level or raw trigger IDs and kept the message at the
row-level configuration boundary.

The direct API path returned HTTP 409 with code
`automation_trigger_not_ready`; the observed unchanged run count was `6 → 6`.
This confirms that a blocked manual run does not create an automation run.
The UI and API evidence were captured against the `ce0d1248` runtime described
above.

## Tools and execution boundary

Cursor's inspected no-repository automation exposed Memories, Send to Slack,
Read Public Slack Channels, and MCP tools. The settings page manages selected
Memories, Send to Slack, and workspace MCP tools. Tool presence means selected:
there are no per-row enable switches. Adding or removing a tool changes its
configuration; removing Memories does not delete stored notes. The executor
picker selects a real Agent; model selection is a separate runtime setting.
Model and MCP selections are applied on task claim.

Memories are named Markdown records in `automation_memory`, outside repository
files and provider-global memory. The UI and `patchbay automation memory
list/read/write/delete` use the same API. Task credentials access only the
running automation while memories are enabled. Human access requires the
existing automation write permission. Revisions reject stale writes, including
after delete/recreate; failed or conflicting UI saves preserve the draft.

Send to Slack uses an explicit connected installation and channel IDs. A
successful completion sends the final output through the native Slack client;
the agent is told not to duplicate that delivery. Failure details are retained
with the run, and the implementation does not silently claim successful
delivery.

The daemon retains configured runtime MCP services and the automation's
selected workspace services. A real run exercised a slash-containing MCP name;
the Codex adapter serializes it as a quoted TOML key, preserving its name and
enabled state without modifying user-global configuration.

## Reproduced gaps and fixes

- Search input no longer loses keystrokes to menu typeahead and searches
  translated labels; no results produce an explicit empty state.
- Existing weekly and interval schedules have a complete edit path that
  preserves timezone and advanced cron validation. Cancel does not write.
- Unconnected trigger rows show only the provider icon, name, `Requires
  connection`, Connect, and delete. Selectors, filters, and per-row enable
  controls appear only after connection.
- Per-row trigger and tool enable switches were removed. Presence/removal is
  the persisted state; the overall automation active/paused control remains.
- Slack reaction filters expose only payload-supported fields; GitHub comment
  text filters are wired.
- Trigger configuration saves serialize concurrent writes; newer edits queue and
  failed drafts remain available for explicit retry.
- Native event records are visible without a generic webhook token, including
  delivery records for native webhook triggers.
- MCP has an onward path from an empty library, selected servers/removal,
  accessible names, and separate loading/error/empty states.
- Connection, model, status, project, schedule, and history errors no longer
  silently resemble empty data or successful writes.
- Linear uses its provider icon. Trigger rows wrap in narrow layouts and use the
  repository's role-based typography scale and existing theme tokens.
- The executor control uses the real Agent picker and persists
  `executor_type`/`executor_id`; it does not present a model list as the agent
  selector.
- Instructions have a visible heading, compact typography, a fixed-height
  scrolling editor, and a model selector outside the scrolling content.
- Desktop titlebar layout follows the pinned sidebar state, preserving native
  traffic-light clearance during hover reveal.
- Narrow Electron windows retain the desktop sidebar instead of opening a mobile
  navigation sheet. The hover-revealed sidebar has an opaque surface and covers
  the overlapping tab strip without moving the main page.
- The obsolete transcript dialog/button, public export, and unused UI graph were
  deleted. Run History, Agent Activity, and issue task entrypoints open the
  Agent conversation in a resizable right pane. The pane's collapse control
  preserves draft text and history; keyboard resizing was verified. The old pin
  control is absent.
- The final continuation checks cover post-GC Antigravity execution, Steer,
  and Stop. The original worktree was absent after GC; the same Antigravity
  session (session ID beginning `6b65…`) completed in a new worktree with
  `PR777_POST_GC_OK`. A Steer task was cancelled, then a follow-up task
  completed with `PR777_STEER_OK`; the explicit Stop task was cancelled.
- Commit `1a71628` removes the persistent trigger-auth summary from the bottom of
  the settings surface. The final-head visual runtime was not rebuilt after the
  later source merge, so this handoff does not claim a post-merge visual
  recheck.

## Acceptance matrix

Primary client: the real Electron canary from the isolated checkout, backed by
its own PostgreSQL database and API. The original main checkout was restored
separately and remains outside this worktree.

| Axis | Cases |
| --- | --- |
| Window | Normal desktop and narrow desktop window |
| Appearance | Light and dark |
| Text/input | Chinese and English labels; keyboard search, navigation, and focus |
| Data | Paused/active, unconnected providers, empty/selected MCP, empty/history records |
| Writes | Daily time, full schedule/timezone, provider additions, filters, instructions, Agent, model, tools |
| Errors | Invalid filter/cron, connection retry, unsaved input preservation, readiness-blocked manual run |
| Continuation | Agent pane collapse, draft/history retention, keyboard resize, post-GC run, Steer, Stop |

Pass requires a visible UI result and a subsequent API read, not only a toast or
a mocked mutation. No real production provider event or outgoing Slack message
is claimed by local fixtures.

## Evidence

The disposable evidence directory contains the Cursor and Electron screenshots,
API readbacks, continuation records, and prior focused logs. The final
continuation screenshots are:

- `electron-antigravity-post-gc-final.png`
- `electron-antigravity-steer-final.png`
- `electron-antigravity-stop-final.png`
- `electron-run-blocked-dialog-final.png`
- `electron-agent-picker-final.png`
- `electron-right-pane-collapse-control.png`
- `electron-right-pane-draft-retained.png`
- `electron-right-pane-resized.png`
- `electron-unconnected-triggers-minimal.png`
- `electron-tab-menu-no-pin.png`

Other confirmed evidence retained from the audit:

- Electron schedule editing produced `TZ=America/New_York 0 8 * * *` and
  `TZ=Asia/Tokyo 0 8 * * 1,3,5` with the same trigger ID and timezone readback.
- Chinese search filtered the provider menu and added
  `linear.issue.created`.
- Memory UI save, close, and reopen retained `PR777_UI_MEMORY_OK`; the agent
  file selector displayed the agent-written `RUN_QA.md` value. Removing and
  re-adding Memories retained the notes and selected MCP service.
- A real configured MCP task wrote and read the memory value across tasks. The
  initial failed MCP run remains recorded with its concrete failure reason and
  was not relabeled as success.
- The Agent picker showed the real workspace Agent list and persisted
  `executor_type=agent` plus `executor_id` through PATCH.
- The right pane collapse control preserved the draft and history; keyboard
  resizing changed the split and kept the main page mounted. Mouse-drag resize
  was not verified.
- Light and dark layouts were inspected at normal and 960 px desktop width.
  The Connect column stayed aligned while longer Slack and Linear conditions
  wrapped.
- Electron's narrow navigation remained a desktop column. The final hover
  screenshot shows the opaque drawer covering the overlapping tab strip while
  the page stays in place and window controls remain available.
- GitHub Connect opened the real GitHub settings destination. This isolated
  deployment reports missing GitHub App configuration; no provider OAuth
  completion is claimed.
- The focused handler evidence includes the assigned readiness pass plus three
  isolated Linear/comment passes. Cloud CI is the authority for the current
  source; no new local test run was performed for this documentation update.

## Delivery state and limits

- PR source merge and conflict resolution completed at head `530d99b6` before
  this documentation commit.
- The current live Electron/API/daemon evidence is from `ce0d1248`, built with
  Go 1.26.7. It has not been rebuilt from `530d99b6` after the source merge.
- Cloud CI run `34143871346` was pending for source head `530d99b6` at this
  handoff; this document does not call it green.
- External OAuth completion, production provider events, outgoing Slack
  delivery, and mouse-drag resize remain outside the verified evidence.
- The docs-only commit and its push update the PR handoff; GitHub PR body edits
  remain a separate final action after the current CI/runtime gate.

## Primary references

- [Cursor Automations](https://cursor.com/docs/cloud-agent/automations)
- [Cursor Automations help](https://cursor.com/help/ai-features/automations)
- [Slack channel_created payload](https://docs.slack.dev/reference/events/channel_created)
- [Slack reaction_added payload](https://docs.slack.dev/reference/events/reaction_added)
