---
name: patchbay-automations
description: "Use when creating, updating, inspecting, triggering, or debugging a Patchbay automation (scheduled, webhook, or manual)."
user-invocable: false
allowed-tools: Bash(patchbay *)
---

# Patchbay Automations

## Quick start

Automations are durable automations. Read before mutating:

```bash
patchbay automation list --output json
patchbay automation get <automation-id> --output json
patchbay automation runs <automation-id> --output json
```

Do not run `trigger`, `delete`, `trigger-delete`, or `trigger-rotate-url` to test. Those are real side effects.

## Core model

An automation is not an agent. It is a rule that dispatches work to an agent, or to a team's leader agent.

The chain is: trigger fires (`schedule`, `webhook`, or `manual`) -> `automation_run` row -> `execution_mode` decides output -> executor readiness check -> issue/task execution -> run status sync. Webhooks have a durable admission step in front: HTTP ingress stores a queued `webhook_delivery`, synchronously creates or reuses its idempotent run, and returns `200` with `status=accepted|skipped` plus `run_id`; a database-leased worker then resumes accepted runs and owns recoverable issue/task dispatch.

Execution modes:

- `create_issue` creates a Patchbay issue, making the run visible as issue state.
- `run_only` creates an agent task directly. No issue is created; any durable
  report location has to come from other task context or instructions.

`issue-title-template` only supports `{{date}}`. Do not invent `{{trigger_id}}`, `{{branch}}`, or other variables.

## CLI

```bash
patchbay automation list --output json
patchbay automation get <automation-id> --output json
patchbay automation create --title "<title>" --description "<task prompt>" --agent <agent-name-or-id> --mode create_issue|run_only --output json
patchbay automation update <automation-id> --status active|paused --output json
patchbay automation runs <automation-id> --output json
patchbay automation trigger-add <automation-id> --kind schedule --cron "0 9 * * *" --timezone Asia/Shanghai --output json
patchbay automation trigger-add <automation-id> --kind webhook --label "ci" --output json
patchbay automation trigger <automation-id> --output json
patchbay automation trigger-rotate-url <automation-id> <trigger-id> --yes --output json
```

Use `trigger` only when the user explicitly asks for a manual run. Use `trigger-rotate-url` only when rotating a webhook URL; the old URL stops being valid.

`automation get` redacts `webhook_token`, `webhook_path`, and `webhook_url` by default while reporting whether a token exists and its non-sensitive hint. Only add `--show-secrets` when the user explicitly asks to retrieve the live webhook credential; the command warns on stderr. Do not paste webhook tokens or signing material into comments, logs, docs, or PRs.

## Debugging

For "why didn't it run":

1. `patchbay automation get <id> --output json` — status, mode, executor, triggers.
2. `patchbay automation runs <id> --output json` — run status and failure reason.
3. If assigned to a team, inspect the team: `patchbay team get <team-id> --output json`; execution goes to the leader.
4. Inspect the target agent/runtime: `patchbay agent get <agent-id> --output json` and `patchbay runtime list --output json`.
5. For webhooks, inspect delivery status: `queued` means the worker has not completed dispatch; `failed` carries the worker error. A provider retry with the same `X-GitHub-Delivery` / `Idempotency-Key` reuses the original delivery.
6. For `create_issue`, inspect the created issue if the run records one.

## Side effects

These mutate durable state or start work: `create`, `update`, `delete`, trigger add/update/delete/rotate, `trigger`, and webhook calls to `/api/webhooks/automations/{token}`.

More source-backed details: `references/automations-source-map.md`.
