# PR #777 continuation handoff

The current implementation and acceptance record is [the automation audit](pr-777-automation-audit.md). This replaces the initial Cursor handoff; its old restrictions against backend changes and an Instructions heading were superseded by the user's later requirements.

## Delivery

- Continue the existing draft [PR #777](https://github.com/alexj11324/Cordy/pull/777), branch `cursor/cloud-agent-1788649529004-9qees`; do not amend merged PR #769.
- Preserve the original checkout and unrelated changes. Work in an isolated checkout, commit verified changes, and push the existing PR branch. Merge requires user authorization.
- Keep one shared Settings page in `packages/views`; Web and Electron entrypoints reuse it.
- Preserve the automation list, capsule tabs, and product terms Run now, No project, and Edit.
- Keep server data in React Query and use the role-based typography scale.
- The last CI-verified pushed head is `5bd6a0442` (`CI 34128280694`). It does not cover the current dirty follow-up; do not treat that older green run as current validation.

## Required behavior

- GitHub, Slack, and Linear triggers use real provider identities and event-specific conditions observed in Cursor. Do not add a generic branch/label panel to PR opened.
- An unconnected trigger row shows only its provider icon, name, `Requires connection`, `Connect`, and delete. It hides selectors, filters, and per-row enable controls until the provider is connected. The overall automation active/paused control remains.
- Run now and every dispatch path require at least one ready trigger. A missing connection or required parameter blocks admission. Legacy persisted `enabled=false` triggers remain visible and blocking until an explicit delete/recreate; reads do not mutate them.
- The executor control selects a real Agent and persists `executor_type`/`executor_id`; it is not a model picker.
- Instructions have a visible heading and a compact fixed-height editor with internal scrolling. The model control remains outside that scroll area.
- Memories are persistent named Markdown notes scoped to an automation, with real UI and task-bound CLI reads/writes. A Markdown file in a checkout alone is not cross-run memory.
- Selected MCP services must be invoked in a real task; a saved checkbox is insufficient evidence.
- Send to Slack uses real connected channel targets and reports delivery failures. Do not send external test messages without explicit authorization.
- Remove the obsolete standalone transcript UI and its fallback exports. Visible execution entrypoints open the interactive Agent conversation; completed, failed, active, interrupt, and steer paths need appropriate verification.
- Collapsing or temporarily revealing the sidebar must preserve its toggle and clear the native traffic lights.
- The Agent conversation pane has a right-side collapse control. Reopening preserves draft text and history; keyboard resize was verified. Mouse-drag resize remains unverified. The obsolete pin control is removed.

## Verification

Use `make up C=api,desktop` and the generated development login. The primary target is Electron; do not substitute a browser preview. Verify UI writes with API readbacks. Use an isolated PostgreSQL test database for handler tests.

The audit records screenshots, focused test results, and real MCP/memory runs. Provider webhook and Slack HTTP fixtures establish local integration behavior; they do not prove that a production provider is configured or that a live external message was delivered. The latest dirty follow-up still needs the full handler rerun, the latest runtime continuation checks, and Antigravity post-GC/Stop/Steer verification.

Before delivery, finish the independent review, the latest interactive Agent-panel runtime checks, the final focused test/typecheck/lint passes, and current PR CI. Keep the goal active until the required evidence is complete.
