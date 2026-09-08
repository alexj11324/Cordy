# Codex: fill native automation secrets

This is an operator checklist, not product design. The fan-out code is already
wired (`automation_native_fanout.go` + `automation_event.go`). Do **not** add
OAuth/API surfaces for these providers. Put the real values in the deployment
environment (and `.env` locally). Never commit secrets.

Until the keys and subscriptions below are present, GitHub/Slack/Linear
workspace webhooks 503 or ignore events. That is expected.

## Env (values only — names already exist)

| Variable | Used for |
|---|---|
| `GITHUB_APP_SLUG` | Connect GitHub button / App URL |
| `GITHUB_WEBHOOK_SECRET` | HMAC on GitHub App webhook deliveries |
| `GITHUB_APP_ID` | App authentication (installation tokens) |
| `GITHUB_APP_PRIVATE_KEY` | PEM for the App (quoted, real newlines) |
| `ORVILO_SLACK_SECRET_KEY` | Encrypt bot tokens at rest (opt-in for Slack) |
| `ORVILO_SLACK_CLIENT_ID` | Hosted Slack OAuth |
| `ORVILO_SLACK_CLIENT_SECRET` | Hosted Slack OAuth |
| `ORVILO_SLACK_SIGNING_SECRET` | HMAC on managed Slack Events API |
| `LINEAR_WEBHOOK_SECRET` | HMAC on Linear workspace webhook |

Placeholders live in `.env.example`. Copy into the running environment; do not
invent parallel variable names.

## GitHub App subscriptions to expand

Keep using Orvilo's own GitHub App (do not reuse Cursor cloud OAuth). On the
App's webhook permissions / subscribed events, add or confirm:

- `pull_request` (opened, reopened, synchronize, closed, labeled, unlabeled)
- `push`
- `issue_comment`
- `issues` (labeled, unlabeled)
- `pull_request_review_comment`
- `pull_request_review`
- `pull_request_review_thread`
- `workflow_run` (completed)
- `check_suite` (completed)

These map to catalog presets in `packages/core/automations/trigger-catalog.ts`.
`ping` stays a no-op. The existing App webhook path is unchanged; fan-out runs
after signature verification.

## Slack Events API

On the Orvilo Slack app (Events API / bot events), subscribe:

- `message.*` (and `app_mention`) → `slack.message`
- `reaction_added` / `reaction_removed` → `slack.reaction`
- `channel_created` → `slack.channel_created`

Managed webhook still requires `ORVILO_SLACK_SIGNING_SECRET`. Empty secret →
503, never unsigned accept.

## Linear webhook

Workspace Linear connection already verifies HMAC with `LINEAR_WEBHOOK_SECRET`.
Confirm the Linear app webhook includes:

- Issue create → `linear.issue.created`
- Issue update (status / stateId in `updatedFrom`) → `linear.issue.status_changed`
- Cycle update with `completedAt` → `linear.cycle.ended`

## What Codex should not do

- Do not mint per-automation public URLs for GitHub/Slack/Linear.
- Do not change `/api/webhooks/automations/{token}` (generic Webhook Triggered).
- Do not add Sentry / PagerDuty / Teams as trigger sources.
- Do not put secrets in this file, SKILL.md, docs, or PRs.
