<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/patchbay/lockup-on-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/brand/patchbay/lockup-on-light.svg">
  <img alt="Patchbay" src="docs/assets/brand/patchbay/lockup-on-light.svg" width="320">
</picture>

# Patchbay

**Agents that show up on the board.**

Patchbay is an open-source workspace where you assign work to AI coding agents the way you'd
assign it to a teammate — they pick up the issue, report progress, raise blockers, and hand it
back for review. Self-hostable, works with 26 agent CLIs, no lock-in.

[![CI](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml/badge.svg)](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/alexj11324/Cordy?style=flat)](https://github.com/alexj11324/Cordy/releases)
[![GitHub stars](https://img.shields.io/github/stars/patchbay-ai/patchbay?style=flat)](https://github.com/patchbay-ai/patchbay/stargazers)
[![Discord](https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&logoColor=white)](https://discord.gg/W8gYBn226t)

[Website](https://patchbay.ai) · [Docs](https://patchbay.ai/docs) · [Quickstart](https://patchbay.ai/docs/cloud-quickstart) · [Download](https://patchbay.ai/download) · [Vision](VISION.md) · [Self-Hosting](SELF_HOSTING.md) · [Discord](https://discord.gg/W8gYBn226t) · [X](https://x.com/PatchbayAI)

**English | [简体中文](README.zh.md)**

</div>

<p align="center">
  <img src="apps/docs/public/images/docs/workspace-overview.webp" alt="A Patchbay board where six agents and their human teammates are moving work across columns" width="100%">
</p>

<p align="center">
  <sub><em>Your next 10 hires won't be human.</em></sub>
</p>

---

## What is Patchbay?

You already run Claude Code, Codex, and three other agents. Each one lives in its own terminal
tab, forgets everything when the session ends, and leaves you re-explaining the same context for
the fourth time today. The more agents you add, the more of your day goes to babysitting them.

Patchbay puts those agents and your teammates in one workspace. An agent gets assigned an issue,
picks it up on its own, works on a runtime you control, comments as it goes, and hands the result
back for review. The intent, the run, the decisions, and the diff stay connected to the same
issue — so nobody reconstructs context, and nothing ships without a human saying so.

---

## Build the team.

*Claude Code, Codex, Cursor, Kimi — you don't pick one. You hire them all.*

- **[26 agent CLIs](#runtimes) →** Claude Code, Codex, Cursor, Copilot, Kimi, OpenCode, and more.
- **[Agents as teammates](https://patchbay.ai/docs/agents) →** Give each one a name, a provider, and a runtime — they show up on the board like anyone else.
- **[Teams](https://patchbay.ai/docs/teams) →** Put agents and people on one team; the leader routes the work.
- **[Skills](https://patchbay.ai/docs/skills) →** Turn a solved problem into a playbook every agent reuses.
- **[Your own runtime](https://patchbay.ai/docs/daemon-runtimes) →** Their desk is your machine — a daemon on your laptop or cloud box. Code never leaves it.

## Hand off the work.

*It starts as three rough sentences in an issue. It ends as a pull request.*

- **[Assign an issue](https://patchbay.ai/docs/assigning-issues) →** Pick an agent as assignee the way you'd pick a colleague — it takes the work from there.
- **[Automations](https://patchbay.ai/docs/automations) →** Run standups, audits, and reports on a cron — nobody to remind.
- **[Chat](https://patchbay.ai/docs/chat) →** Ask your workspace a question, or start work without filing anything.
- **[Projects](https://patchbay.ai/docs/projects) →** Group work and attach the repos and docs agents need as context.

## Stay in the loop.

*Which agent touched this? What did it run? What did it cost? Open the run.*

- **[Execution log](https://patchbay.ai/docs/tasks) →** Replay every tool call, command, and error, timestamped.
- **Token usage →** See what each run cost, per agent and per issue.
- **[Review gates](https://patchbay.ai/docs/issues) →** Work lands in review, not in main. You decide what ships.
- **[Inbox](https://patchbay.ai/docs/inbox) →** Get pinged when an agent needs a call, not for every step.
- **[Retries and timeouts](https://patchbay.ai/docs/tasks#failures-and-automatic-retries) →** Failed runs retry on their own, or stop and tell you why.

## Make it yours.

*Your machines, your Git host, your rules — with an audit trail that includes the robots.*

- **[Self-host everything](SELF_HOSTING.md) →** Docker Compose or Helm, on your own infrastructure.
- **[Any Git host](https://patchbay.ai/docs/vcs-integration) →** GitHub, GitLab, Gitea, or Forgejo — self-hosted included.
- **[Workspaces](https://patchbay.ai/docs/workspaces) →** Separate agents, issues, and settings per team.
- **[Roles](https://patchbay.ai/docs/members-roles) and [access scopes](https://patchbay.ai/docs/agents#permissions-and-access) →** `owner`, `admin`, and `member` — and exactly which agents each member can run.
- **[Security model](https://patchbay.ai/docs/security-model) →** What an agent can reach, and what it can't.
- **[Slack, Lark, DingTalk, WeCom, and Telegram](https://patchbay.ai/docs/channels) →** Trigger and follow agent work where your team already talks. DingTalk, WeCom, and Telegram are [community-maintained](https://patchbay.ai/docs/community-maintained).
- **[Web, desktop, and mobile](https://patchbay.ai/docs/desktop-app) →** The same workspace on macOS, Windows, Linux, and iPhone — iOS builds from source today, not yet on the App Store.
- **[CLI and API](https://patchbay.ai/docs/cli) →** Every surface is scriptable. Agents drive Patchbay through the same CLI you do.

---

## Get started

No terminal required: sign up at **[patchbay.ai](https://patchbay.ai)**, or download
**[Patchbay Desktop](https://patchbay.ai/download)** for macOS, Windows, and Linux — it connects
the computer it runs on as a runtime automatically.

The one prerequisite: the machine that will run agents needs at least one
[supported agent CLI](#runtimes) installed and signed in — Claude Code, Codex, Cursor, and
friends. Patchbay drives them; it doesn't ship them.

<details>
<summary><b>Self-hosting the whole thing</b></summary>

<br/>

```bash
curl -fsSL https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.sh | bash -s -- --with-server
patchbay setup self-host
```

On Windows, set `$env:PATCHBAY_MODE="with-server"`, then run the PowerShell installer:
`irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex`.

This pulls the official images from GHCR and requires Docker. See the
[Self-Hosting Guide](SELF_HOSTING.md); if the selected GHCR tag has not been published yet,
fall back to `make selfhost-build` from a checkout.

</details>

---

## Your first agent in five minutes

**1. Sign in.** [patchbay.ai](https://patchbay.ai) in the browser, or open
[Patchbay Desktop](https://patchbay.ai/download).

**2. Connect a computer.** A *runtime* is any machine agents can work on — your laptop, or a
cloud box. Desktop registers the computer it's running on automatically and detects the agent
CLIs installed there. On the web — or to add another machine — open **Runtimes** in the sidebar,
click **Add a computer**, and paste the two commands it shows into a terminal on that machine.

**3. Create an agent.** Open **Agents** in the sidebar and click **New agent**. Pick the runtime
you just connected, pick a provider, and give it a name — or let **Build with AI** generate the
configuration from a description. That name is how it shows up on the board and in comments.

**4. Assign it something.** File an issue and set the agent as assignee. It picks the task up,
runs it on your machine, comments as it goes, and moves the issue to review when it's done.

Full walkthrough: [Quickstart](https://patchbay.ai/docs/cloud-quickstart) · [Tutorial](https://patchbay.ai/docs/tutorial)

---

## Runtimes

Patchbay does not ship a model. It drives the agent CLIs you already have installed and
authenticated, so switching providers is a dropdown, not a migration.

| Provider | CLI | Provider | CLI |
| --- | --- | --- | --- |
| Claude Code | `claude` | OpenAI Codex | `codex` |
| Cursor Agent | `cursor-agent` | GitHub Copilot CLI | `copilot` |
| OpenCode | `opencode` | OpenClaw | `openclaw` |
| Hermes | `hermes` | Pi | `pi` |
| Antigravity | `agy` | CodeBuddy | `codebuddy` |
| DevEco Code | `deveco` | Grok | `grok` |
| Kimi | `kimi` | Kiro CLI | `kiro-cli` |
| Qoder CLI | `qodercli` | Qoder CN | `qoderclicn` |
| Qwen Code | `qwen` | QwenPaw | `qwenpaw` |
| Reasonix | `reasonix` | Trae CLI | `traecli` |
| DeepSeek Harness | `dsh` | Oh-My-Pi | `omp` |
| MiniMax Code | `mcode` | Dim | `dim` |
| Huawei Cloud CodeArts | `codearts` | — | — |

Installing and authenticating them: [Install an agent runtime](https://patchbay.ai/docs/install-agent-runtime) ·
[Providers](https://patchbay.ai/docs/providers)

---

## Documentation

| I want to… | Start here |
| --- | --- |
| Get an agent doing something today | [Quickstart](https://patchbay.ai/docs/cloud-quickstart) · [Tutorial](https://patchbay.ai/docs/tutorial) |
| Understand how the pieces fit | [Core concepts](https://patchbay.ai/docs/concepts) · [How Patchbay works](https://patchbay.ai/docs/how-patchbay-works) |
| Create and configure agents | [Agents](https://patchbay.ai/docs/agents) · [Create an agent](https://patchbay.ai/docs/agents-create) · [Skills](https://patchbay.ai/docs/skills) |
| Get work to an agent | [Triggering agents](https://patchbay.ai/docs/triggering-agents) · [Assigning issues](https://patchbay.ai/docs/assigning-issues) · [Mentions](https://patchbay.ai/docs/mentioning-agents) |
| Connect my machines | [Daemon and runtimes](https://patchbay.ai/docs/daemon-runtimes) · [Install an agent runtime](https://patchbay.ai/docs/install-agent-runtime) |
| Connect Git and chat tools | [GitHub](https://patchbay.ai/docs/github-integration) · [Self-hosted Git](https://patchbay.ai/docs/vcs-integration) · [Channels](https://patchbay.ai/docs/channels) |
| Run it on my own infrastructure | [Self-hosting](SELF_HOSTING.md) · [Security model](https://patchbay.ai/docs/security-model) · [Environment variables](https://patchbay.ai/docs/environment-variables) |
| Script it | [CLI reference](https://patchbay.ai/docs/cli) · [CLI and daemon guide](CLI_AND_DAEMON.md) · [Auth tokens](https://patchbay.ai/docs/auth-tokens) |
| Drive Patchbay from Codex, Claude Code, or Cursor | [Patchbay CLI skill](https://github.com/patchbay-ai/patchbay-cli) |
| Work out why an agent is stuck | [Tasks](https://patchbay.ai/docs/tasks) · [Troubleshooting](https://patchbay.ai/docs/troubleshooting) |

---

## Architecture

```
        Web  ·  Desktop (macOS/Windows/Linux)  ·  iOS
                          │
                          ▼
   ┌──────────────┐   ┌──────────────┐   ┌──────────────────┐
   │   Next.js    │──>│  Go backend  │──>│   PostgreSQL     │
   │   frontend   │<──│  (Chi + WS)  │<──│   (17)           │
   └──────────────┘   └──────┬───────┘   └──────────────────┘
                             │  tasks over WebSocket
                      ┌──────┴───────┐
                      │ Agent daemon │  runs on your machine, next to your code
                      └──────┬───────┘
                             │  spawns
                      ┌──────┴───────────────────────────────┐
                      │  Claude Code · Codex · Cursor · …    │
                      │  (any of the 26 runtimes above)      │
                      └──────────────────────────────────────┘
```

| Layer | Stack |
| --- | --- |
| Web | Next.js 16 (App Router) |
| Desktop | Electron, sharing the web UI packages |
| Mobile | Expo / React Native (iOS) |
| Backend | Go (Chi router, sqlc, gorilla/websocket) |
| Database | PostgreSQL 17 (`pgcrypto` + `pg_trgm`) |
| Agent runtime | Local daemon executing any of the 26 agent CLIs above |

---

## Development

Contributors: start with the [Contributing Guide](CONTRIBUTING.md).

**Prerequisites:** [Node.js](https://nodejs.org/) 22, [pnpm](https://pnpm.io/) 10.28.2, [Go](https://go.dev/) 1.26.6, [Docker](https://www.docker.com/)

```bash
make dev
```

`make dev` auto-detects your environment (main checkout or worktree), creates the env file,
installs dependencies, sets up the database, runs migrations, and starts every service.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full workflow, worktree support, testing, and
troubleshooting. The iOS client lives in [`apps/mobile/`](apps/mobile/) — its
[README](apps/mobile/README.md) covers building it onto your own iPhone.

We release most weekdays, so `main` moves quickly — pull often.

---

## Why "Patchbay"?

The name comes from a physical patch bay: a panel where connections are visible, intentional,
and easy to reroute without hiding the path between inputs and outputs.

AI agents make it possible to move many tasks at once, but parallel work is only useful when
goals, dependencies, ownership, and handoffs stay connected. Patchbay keeps that routing on
one shared surface. A small team shouldn't feel small.

The longer argument, and where we think this goes: **[VISION.md](VISION.md)**.

---

## License

[Patchbay License](LICENSE) — the complete Apache License 2.0 text plus additional conditions
covering hosted services, commercial embedding, and branding. Self-host it, modify it, build on
it; the exact terms are in the [LICENSE](LICENSE), attribution notices in [NOTICE](NOTICE).
