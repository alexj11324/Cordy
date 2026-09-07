# PR #777: automation trigger and interaction audit

Continuation of PR #777, audited against the signed-in Cursor Automations UI on
2026-09-06/07 and the current Electron canary. The last CI-verified pushed head
is `5bd6a0442` (`CI 34128280694`); the current follow-up is dirty and requires
its own validation. This document replaces the older handoff's remaining-work
list. The user's later requirements include persistent memories, actual MCP
execution, provider-specific conditions, and removal of the obsolete read-only
transcript interface.

## Trigger semantics

A trigger starts the automation's assigned agent or team with the configured
instructions and event payload. The existing execution mode decides whether
to run a task directly (`run_only`) or create an issue (`create_issue`). The
project supplies execution context. A GitHub repository filter controls which
repository's events may start it; choosing an execution project alone does not
filter event sources.

Trigger readiness is server-owned admission. An automation with zero triggers,
an unconnected provider, a missing required parameter, or a persisted legacy
disabled trigger cannot run. The read path reports the reason without rewriting
the trigger. For Slack `channel_created`, an omitted `installation_id` keeps
the existing Any-workspace wildcard semantics and is ready when the workspace
has an installed, nonpaused Slack connection; it does not require a channel.
Provider lookup failures are returned as errors. Durable webhook deliveries stay
queued and retry instead of becoming terminal skipped runs.

| Source | Event choices inspected in Cursor | Cordy implementation |
| --- | --- | --- |
| Schedule | Scheduled runs | Full time, interval, weekly/monthly, timezone and advanced cron editor, now available for each existing schedule as well as creation |
| GitHub | Draft opened; PR opened/pushed/merged; comments; branch push; labels; CI; issue/review comments; submitted reviews; review threads; workflow completion | 14 presets. Source repositories are independent of execution project. Conditions belong to their specific event, including PR authors, pushed branch, changed label, review result, thread state, and workflow conclusion |
| Slack | Channel messages, reactions, channel creation | Connected rows expose installation/channel conditions; unconnected rows show only connection status and Connect. `channel_created` supports Any connected workspace; message/reaction support keyword or regex, authenticated-sender and thread-reply conditions, and optional completion reaction. Channel-created object payload parses correctly. Separate reactions on one message remain distinct while retries deduplicate |
| Linear | Issue created, issue status changed, cycle ended | Real team/project/status IDs from the connected provider; filters match the corresponding webhook fields |
| Webhook | Webhook triggered | Existing generated URL, copy, rotation and delivery/replay paths retained |

Cursor's GitHub submenus explicitly offer review results (approved, changes
requested, commented, any), thread states (resolved, unresolved, any), and
workflow conclusions (success, failure, cancelled, any). Cordy now exposes
these only on the corresponding trigger. Legacy saved filters retain their
backend meaning. Unsupported field/preset combinations are rejected at save time.

Cursor's PR-opened row contains repositories and author, without an extra
branch/label panel. That extra panel was removed. Cursor's branch-push row does
contain a repository, branch, and author. New Slack message triggers default to
ignoring thread replies; existing saved configurations preserve their meaning.
CI uses check-suite completion rather than firing independently for each check.

## Tools and execution boundary

Cursor's inspected no-repository automation exposed Memories, Send to Slack,
Read Public Slack Channels, and MCP tools. Its official documentation also
describes PR comments and reviewer requests when repository context is available.

The settings page manages selected Memories, Send to Slack, and workspace MCP
tools. Tool presence means selected: there are no per-row enable switches.
Adding/removing a tool changes its configuration; removing Memories does not
delete stored notes. The executor picker selects a real Agent; model selection
is a separate runtime setting. Model and MCP selections are applied on task
claim.

Memories are named Markdown records in `automation_memory`, outside repository
files and provider-global memory. The UI and `patchbay automation memory
list/read/write/delete` use the same API. Task credentials can access only their
own running automation while memories are enabled. Human access requires the
existing automation write permission. Revisions reject stale writes, including
after delete/recreate; failed or conflicting UI saves preserve the draft.

Send to Slack uses an explicit connected installation and channel IDs. Successful
completion sends the final output through the native Slack client. The agent is
told not to duplicate that delivery. Failure details are retained with the run;
the implementation does not silently claim successful delivery.

The daemon retains configured runtime MCP services and the automation's selected
workspace services. A real run exposed rejection of a valid slash-containing
MCP name. The Codex adapter now serializes it as a quoted TOML key, preserving
its name and enabled state without modifying user-global configuration.

## Reproduced gaps and fixes

- Search input lost keystrokes to menu typeahead and did not search translated
  labels. Typing is now retained; Chinese labels are searchable; no results
  produces an explicit empty state.
- Existing weekly/interval schedules had no complete editing path. A shared
  create/edit dialog now writes the existing trigger, preserving timezone and
  advanced cron validation. Cancel does not write.
- Unconnected trigger rows were exposing controls that could not work. They now
  show only the provider icon, name, `Requires connection`, Connect, and delete;
  selectors, filters, and per-row enable controls appear only after connection.
- Per-row trigger and tool enable switches were removed. Presence/removal is the
  persisted state; the overall automation active/paused control remains.
- Slack reaction filters offered a keyword the backend rejects. Only
  payload-supported fields are shown; GitHub comment text filters are wired.
- Trigger configuration PATCH requests could race. One save runs at a time,
  newer edits queue, and failed drafts remain available for explicit retry.
- Native event records were hidden because the page required a generic webhook
  token. All native webhook triggers now expose the existing delivery records.
- MCP had no onward path from an empty library and did not show selected servers
  in the tools list. It now has a real settings link, selected rows/removal,
  accessible names, and separate loading/error/empty states.
- Connection, model, status, project, schedule, and history errors no longer
  silently resemble empty data or successful writes.
- Linear uses its provider icon. Trigger rows wrap in narrow layouts; typography
  uses the repository's role-based scale and existing theme tokens.
- The executor control now uses the real Agent picker and saves
  `executor_type`/`executor_id`; it does not present a model list as the agent
  selector. The Linear icon uses the supplied SVG paths and currentColor.
- Instructions have a visible heading, compact typography, a fixed-height
  scrolling editor, and a model selector outside its scrolling content.
- Desktop titlebar layout follows the pinned sidebar state, so temporary hover
  reveal cannot remove the space reserved for native traffic lights and controls.
- Narrow Electron windows retain the desktop sidebar instead of opening the
  mobile navigation sheet. The hover-revealed sidebar has an opaque surface;
  native glass remains on the pinned sidebar. Its overlay must cover the tab
  strip where they overlap without moving the main page.
- The obsolete transcript dialog/button, their public export, and the unused UI
  graph have been deleted. Run History, Agent Activity, and issue task entrypoints
  now open the Agent conversation in a resizable right pane. The main page stays
  visible and retains its mounted state; the conversation and tool events scroll
  inside their pane. The floating chat button and its shortcut are suppressed
  while this pane is open on the same route. The source guard rejects restoring
  the deleted components or imports.
- Direct automation and quick-create conversations use the existing Agent
  continuation lifecycle. Follow-ups retain parent provenance and scoped
  capabilities without rerunning the original automation or one-shot issue
  creation. Runtime acceptance of the latest continuation implementation,
  Stop, and Steer remains pending; Antigravity post-GC verification is also
  pending.

## Acceptance matrix

Primary client: the real Electron canary from this isolated checkout, backed by
its own PostgreSQL database and API. The original main checkout is untouched.

| Axis | Cases |
| --- | --- |
| Window | Normal desktop and narrow desktop window |
| Appearance | Light and dark |
| Text/input | Chinese and English labels; keyboard search, navigation and focus |
| Data | Paused/active, unconnected providers, empty/selected MCP, empty/history records |
| Writes | Daily time, full schedule/timezone, three provider additions, filters, instructions, Agent, model, tools |
| Errors | Invalid filter/cron, connection retry, unsaved input preservation |

Phone orientation and native Dynamic Type do not apply: this is the shared
Electron/Web detail surface; mobile is a separate client. Ordinary window
reflow and text zoom cover the applicable constrained-space checks. The
automation list, capsule tabs, and product terms are preserved.

Pass requires a visible UI result and a subsequent API read, not only a toast or
a mocked mutation. No real production provider event or outgoing Slack message
is claimed by local fixture tests.

## Evidence

Local evidence directory: `$HOME/.cache/codex-disposable/pr777-automation-audit/`.
It contains Cursor screenshots (`cursor-*.png`), Electron before/after and
interaction screenshots (`electron-*.png`), API readbacks, and test logs.
Screenshots remain local because the signed-in Cursor reference contains
unrelated private conversation titles.

Confirmed during the continuation:

- Electron 07:00 → 08:00 produced `TZ=America/New_York 0 8 * * *` in GET detail.
- Full schedule editing produced `TZ=Asia/Tokyo 0 8 * * 1,3,5` and
  `timezone=Asia/Tokyo` on the same trigger ID.
- A connection query failed during the local API restart; its visible Retry
  action recovered to the actual unconnected state.
- Chinese search filtered the provider menu and added `linear.issue.created`.
- Focused handler tests used a separate real PostgreSQL database to verify
  native event delivery rows and retry deduplication.
- Memory UI save → close → reopen retained `PR777_UI_MEMORY_OK` in `MEMORIES.md`.
- The memory file selector displayed the agent-written `RUN_QA.md` value. Removing
  and re-adding Memories retained the notes and the selected MCP service. Adding
  Send to Slack without an installation saved a disabled tool and showed the
  actual Connect route instead of an enabled unusable destination.
- Unconnected trigger rows were visually reduced to provider icon/name,
  `Requires connection`, Connect, and delete. Connected rows retain their
  provider-specific selectors and filters.
- The Agent picker showed the real workspace Agent list and persisted
  `executor_type=agent` plus `executor_id` through PATCH. It did not use a model
  list for executor selection.
- The right Agent pane collapse control hid and restored the pane while
  preserving draft text and history. Keyboard resizing changed the split and
  kept the main page mounted. A mouse-drag resize attempt did not change the
  split and is not claimed as verified. The old pin control is absent.
- Focused readiness tests passed for zero/incomplete triggers, Slack
  `channel_created` Any-workspace readiness, provider lookup error propagation,
  and durable webhook retry. The current dirty full handler run is still
  pending; a prior fixture-adapted run had only unrelated continuation/memory
  regressions and a nondeterministic Linear result, while the isolated Linear
  suite passed.
- Light and dark layouts were inspected at normal and 960 px desktop width.
  The Connect column stays aligned while longer Slack and Linear conditions wrap.
- Electron's narrow navigation opens as a desktop column, not a mobile sheet.
  The final hover screenshot `electron-hover-sidebar-above-tabs.png` shows the
  opaque drawer covering the overlapping tab strip while the page stays put and
  window controls remain available. This concern is committed as `e13ad085c`.
- GitHub Connect opened the real GitHub settings tab. This isolated deployment
  truthfully reports its missing GitHub App configuration; no provider OAuth
  completion is claimed for it.
- Run `01a07937-f9eb-7b1b-9dbf-bb632434f1af` called the actual configured
  `pr777-qa-echo` MCP tool and saved its value into `RUN_QA.md`, revision 1.
- A second task, run `01a0793b-1255-7196-8980-f77242857f8f`, used a different
  provider session and read that value through the memory CLI. Its prompt did
  not contain the expected value; tool output and the final response matched
  the persisted record. `run2-proof.json` records this check.
- The initially failed MCP run is retained in history with its concrete failure
  reason; it was not relabeled as a success.
- Cursor inspection changes were never saved. Reloading the verified reference
  automation removed the temporary trigger rows and left Save disabled.

The latest runtime verification is still open. Antigravity post-GC continuation,
Stop, and Steer have not been accepted. No external OAuth completion, production
provider event, or outgoing Slack message is claimed.

## Primary references

- [Cursor Automations](https://cursor.com/docs/cloud-agent/automations)
- [Cursor Automations help](https://cursor.com/help/ai-features/automations)
- [Slack channel_created payload](https://docs.slack.dev/reference/events/channel_created)
- [Slack reaction_added payload](https://docs.slack.dev/reference/events/reaction_added)
