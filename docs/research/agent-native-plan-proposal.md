# Proposal: one request, one visible plan

Status: interaction direction approved by the user; revised visual prototype and production implementation pending.
Date: 2026-09-05

## Confirmed direction

The user wants a Linear-like, restrained interface. The most common entry is one sentence describing a need. The agent should understand the request, resolve consequential ambiguity using short choices when possible, split the work into tasks, and present the plan visually. Reduce human-authored text and repeated context entry. The first styling iteration in PR #760 did not satisfy this direction.

The user explicitly approved the conversation-first composition: the request conversation is the main surface; the decomposition plan is embedded in that conversation; task details expand only when selected; confirmation creates tasks and a separate Start action begins execution. The same embedded plan shows subsequent progress.

The full-page dependency canvas with a permanently visible right-hand detail panel was explicitly rejected. It is not an implementation target. Preserve the previously approved black/white component character and Linear-like information density; minimalism means fewer decisions and repeated inputs, not excessive empty space.

## Approved interaction sequence

1. **Express the outcome.** One primary input accepts a sentence, a screenshot, or pasted material. The current project supplies context, shown compactly and editable. The user does not first choose a manual/agent mode or fill out task metadata.
2. **Clarify only what changes the plan.** Read available context before asking. A question offers two or three concrete alternatives and permits a custom answer. Routine defaults remain visible assumptions; consequential ambiguity stays unresolved until answered. Do not ask a fixed questionnaire for every request.
3. **Show the proposed work inside the conversation.** Embed one compact, interactive plan alongside the agent response. Each task initially shows its title and a concise relationship cue. Express sequence and parallel work within that plan; do not navigate to a separate full-page graph editor. Selecting a task expands its output, acceptance criteria and context on demand, preferably within the plan. No permanent right-hand detail panel.
4. **Adjust in place.** Let the user select a task and refine it, remove it, or ask the agent to merge/split/reorder work. The agent should describe meaningful changes and preserve stable task identity across revisions. A request can be answered without generating tasks if it is only a question; a simple actionable request may produce one task.
5. **Start at a clear boundary.** Confirmed user decision: accepting the plan creates tasks only. The tasks remain not started until the user separately chooses Start. A draft plan must never show live execution states. Confirmation must refer to the current revision, not an earlier plan.
6. **Keep the embedded plan during execution.** The same task entries gain working / waiting for your answer / blocked / ready-to-review states, driven by real events. Running work can be steered when the provider supports it. Logs live behind details. Results expose previews, diffs or documents with verified and unverified outcomes.

Example: “让应用支持一致的黑白两套主题” can become inspect current surfaces → shared component theme and native window appearance in parallel → verify and deliver previews. This is an illustrative proposal, not an instruction to execute those tasks.

## Why these choices

- Linear's structured option elicitation and context-aware repository suggestions support short, consequential questions instead of prompt-writing homework. See [agent interaction](https://linear.app/developers/agent-interaction) and [signals](https://linear.app/developers/agent-signals).
- Cursor Plan Mode documents research/clarification, an editable plan and a transition into building. This supports a deliberate plan-to-execution boundary, without requiring planning for trivial work. See [Plan Mode](https://cursor.com/docs/agent/plan-mode).
- Anthropic describes bringing the agent to the user's files/tools and asking for the desired outcome, reducing copy/paste between chat and work. This supports context capture and artifact-oriented results. See [Cowork best practices](https://claude.com/blog/best-practices-for-getting-started-with-claude-cowork).
- These are documented patterns, not comparative usability results. Automatic decomposition into Cordy task graphs is our proposed product behavior. Do not claim a measured reduction in effort before testing it.

The companion [research comparison](agent-native-interaction-patterns.md) contains direct primary sources and capability limits.

## Current source evidence and engineering boundary

- `packages/views/modals/quick-create-issue.tsx` currently submits `api.quickCreateIssue` with an explicitly selected agent/team. On acceptance it saves those preferences and shows a sent notification. A persistent, reviewable multi-task plan is not returned by this UI path.
- `packages/core/types/dependency-graph.ts` already models goals, task descriptions, acceptance criteria, outputs, candidate executors, dependency edges and waves. `packages/views/task-graph` provides graph/list views of persisted plans. Reuse these domain capabilities where appropriate.
- `server/internal/handler/dependency_graph.go:ApplyIssueDependencyGraph` creates graph issues transactionally, then calls `WakeDependencyGraphReadyTasks`. Existing apply is therefore an execution-affecting operation; it is not a read-only draft preview API.
- `server/internal/service/builtin_skills/patchbay-onboarding/SKILL.md` already instructs Patrick to clarify a real goal, preview and confirm before creating work. The proposed interaction makes that product-wide and explicit in the interface rather than relying only on prose instructions.
- The inspected Codex adapter `handleServerRequest` does not currently expose a complete end-user choice-question lifecycle: its MCP elicitation case accepts with nil content, and unhandled request types error. A provider-independent question/answer contract needs separate design and verification before promising native option handling for every agent.

These findings are static/source evidence. They are not a verified implementation of the proposed flow.

## Proposed first implementation slice

One end-to-end path: sentence → necessary choice → draft plan → revision → explicit confirmed transition. Start with the existing project context and one supported agent/provider. Avoid adding a new sidebar category, separate prompt templates, autonomous task routing, or a general workflow builder merely to demonstrate the path.

Acceptance:

- A user describes an outcome once; title, descriptions and initial metadata are generated without retyping.
- The plan is embedded in the request conversation, shows real proposed task identities/dependencies, and supports a scoped correction.
- Details appear only on selection; the default view has no full-page canvas or permanent detail sidebar.
- Clarification, proposal, task creation and execution progress preserve the same conversation and plan rather than requiring the user to re-enter the request.
- Draft display creates no formal child tasks. Confirm creates the current revision's tasks once without admitting agent runs; a separate Start action admits the eligible work.
- Missing information and rejected actions remain actionable and visible; no simulated success or fabricated running states.
- The same scenario is visually reviewed in light/dark and narrow/wide layouts before calling it complete.

Confirmed transition: **draft plan → Confirm and create tasks → created, not started → Start → execution**. Creating tasks and dispatching work must be separately represented and independently verified. The current dependency-graph apply/wakeup coupling cannot serve the first transition unchanged.

Suggested primary-action labels: “创建 4 个 tasks” while reviewing a four-task proposal; “开始” after creation. The exact task count comes from the current proposal, not fixed copy. Keep the same visual plan in both states.

The interaction direction above is approved. A revised visual prototype matching that composition still needs review; the rejected full-page graph image must not be used as visual acceptance. No new interaction implementation is claimed.
