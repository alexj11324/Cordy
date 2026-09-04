# CLI and Agent Daemon Guide

The `patchbay` CLI connects your local machine to Patchbay. It handles authentication, workspace management, issue tracking, and runs the agent daemon that executes AI tasks locally.

## Installation

### Homebrew (macOS/Linux)

```bash
brew install alexj11324/Cordy/patchbay
```

### Build from Source

```bash
git clone https://github.com/alexj11324/Cordy.git
cd Cordy
make build
cp server/bin/patchbay /usr/local/bin/patchbay
```

### Update

```bash
brew upgrade alexj11324/Cordy/patchbay
```

For install script or manual installs, use:

```bash
patchbay update
```

`patchbay update` auto-detects your installation method and upgrades accordingly.

## Quick Start

```bash
# One-command setup: configure, authenticate, and start the daemon
patchbay setup

# For self-hosted (local) deployments:
patchbay setup self-host
```

Or step by step:

```bash
# 1. Authenticate (opens browser for login)
patchbay login

# 2. Start the agent daemon
patchbay daemon start

# 3. Done — agents in your watched workspaces can now execute tasks on your machine
```

`patchbay login` automatically discovers all workspaces you belong to and adds them to the daemon watch list.

## Authentication

### Browser Login

```bash
patchbay login
```

Opens your browser for OAuth authentication, creates a 90-day personal access token, and auto-configures your workspaces.

### Token Login

```bash
patchbay login --token <pby_...>
```

Authenticate using a personal access token directly. Useful for headless environments. Pass `--token=` with an empty value to be prompted interactively (so the token never lands in shell history).

### Check Status

```bash
patchbay auth status
```

Shows your current server, user, and token validity.

### Logout

```bash
patchbay auth logout
```

Removes the stored authentication token.

## Agent Daemon

The daemon is the local agent runtime. It detects available AI CLIs on your machine, registers them with the Patchbay server, and executes tasks when agents are assigned work.

### Start

```bash
patchbay daemon start
```

By default, the daemon runs in the background and writes its log into the state
directory of the profile it was started with — **not always `~/.patchbay/`**:

| Profile | State directory |
| --- | --- |
| Default (no `--profile`) | `~/.patchbay/` |
| Named (`--profile <name>`) | `~/.patchbay/profiles/<name>/` |

That directory holds `daemon.log` (the log), `daemon.pid` (the background
daemon's PID), and `daemon.err.log` (raw crash output; near-empty on a healthy
daemon, since normal logging goes to `daemon.log`).

The Desktop app runs its own named profile, so on a machine that has ever run
both, `~/.patchbay/daemon.log` and `~/.patchbay/profiles/<name>/daemon.log` both
exist and both read as plausible logs — only one is being written to. Don't
guess: `patchbay daemon logs` prints the absolute path it resolved (see
[Logs](#logs)).

To run in the foreground (useful for debugging):

```bash
patchbay daemon start --foreground
```

#### Following a replaced binary

A CLI-launched daemon periodically compares its own compile-time version against
the `--version` output of the `patchbay` binary it would re-exec. When they differ
— `brew upgrade patchbay`, a re-download, a local `make build` — it waits for any
running task to finish, then restarts into the new binary. A running task is
never interrupted; if the daemon is busy the restart is deferred to the next
check, and `patchbay daemon status` shows why it's still on the old version.

This is separate from the GitHub self-update poller: disabling that does not stop
the daemon from following a binary you installed yourself. To turn it off:

```bash
PATCHBAY_DAEMON_AUTO_RELOAD=0 patchbay daemon start
# or
patchbay daemon start --no-auto-reload
# or persist it
patchbay config set disable_auto_reload true
```

Agent CLIs (codex, claude, ...) are handled differently: when one of them is
upgraded in place, the daemon re-probes its version and re-registers the runtime
**without restarting**, so subsequent tasks pick up the new CLI while Patchbay's
availability stays independent of a third party's release cadence.

Desktop-managed daemons ignore both, because the Desktop app owns its bundled
CLI's lifecycle.

### Stop

```bash
patchbay daemon stop
```

### Status

```bash
patchbay daemon status
patchbay daemon status --output json
```

Shows PID, uptime, detected agents, and watched workspaces.

### Logs

```bash
patchbay daemon logs              # Last 50 lines
patchbay daemon logs -f           # Follow (tail -f)
patchbay daemon logs -n 100       # Last 100 lines
patchbay daemon logs --profile staging
```

Every run first prints the absolute path it resolved, so you always know which
profile's log you are looking at:

```
$ patchbay daemon logs -n 100
Reading /Users/you/.patchbay/profiles/desktop-mbp/daemon.log (profile: desktop-mbp)
...
```

That line goes to stderr, before the tail starts — so it also shows up under
`-f`, and piping or redirecting the command still yields log content only:

```bash
patchbay daemon logs -n 500 | grep ERROR   # the path line is not in the pipe
```

Without `--profile`, the default profile's log is read. If it doesn't exist the
command says so and names the path it looked for, which is the fastest way to
find out that the daemon you care about is running on a different profile —
`patchbay daemon status --profile <name>` confirms which one is live.

### Supported Agents

The daemon auto-detects these AI CLIs on your PATH:

| CLI | Command | Description |
|-----|---------|-------------|
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code) | `claude` | Anthropic's coding agent |
| [Antigravity CLI](https://antigravity.google/docs/cli-install) | `agy` | Google Antigravity CLI |
| [CodeBuddy Code](https://www.codebuddy.ai/docs/cli/quickstart) | `codebuddy` | Tencent CodeBuddy Code (reads `CODEBUDDY.md`, not `CLAUDE.md`) |
| [Huawei Cloud CodeArts](https://support.huaweicloud.com/usermanual-cli/codeartsagent_cli_0001.html) | `codearts` | Huawei Cloud coding agent (OpenCode-compatible JSON protocol) |
| [DevEco Code](https://gitcode.com/openharmony-sig/deveco-code) | `deveco` | OpenHarmony DevEco Code |
| [Codex](https://github.com/openai/codex) | `codex` | OpenAI's coding agent |
| [GitHub Copilot CLI](https://docs.github.com/en/copilot) | `copilot` | GitHub's coding agent (model routed by your GitHub entitlement) |
| OpenCode | `opencode` | Open-source coding agent |
| OpenClaw | `openclaw` | Open-source coding agent |
| Hermes | `hermes` | Nous Research coding agent |
| [Pi](https://pi.dev/) | `pi` | Pi coding agent |
| Oh-My-Pi | `omp` | Oh-My-Pi coding agent (Pi fork) |
| [Cursor Agent](https://cursor.com/) | `cursor-agent` | Cursor's headless coding agent |
| Kimi | `kimi` | Moonshot coding agent |
| [Reasonix](https://github.com/esengine/DeepSeek-Reasonix) | `reasonix` | DeepSeek-focused ACP coding agent (run `reasonix setup` first) |
| Dim | `dim` | DimCode ACP coding agent (speaks the ACP protocol via `dim acp`) |
| Kiro CLI | `kiro-cli` | Kiro ACP coding agent |
| [Qoder CLI](https://docs.qoder.com/) | `qodercli` | Qoder ACP coding agent |
| [Qoder CN CLI](https://help.aliyun.com/en/lingma/qodercli-cn/product-overview/what-is-qoder-cli-cn) | `qoderclicn` | Qoder CN ACP coding agent |
| [Trae](https://docs.trae.cn/cli) | `traecli` | ByteDance TRAE CLI (ACP via `traecli acp serve`) |
| [Grok Build CLI](https://docs.x.ai/) | `grok` | xAI Grok Build CLI (ACP via `grok agent stdio`) |
| [Qwen Code](https://github.com/QwenLM/qwen-code) | `qwen` | Alibaba Qwen Code (`qwen -p` with stream-json) |
| [QwenPaw](https://github.com/agentscope-ai/QwenPaw) | `qwenpaw` | QwenPaw ACP coding agent (ACP via `qwenpaw acp`; model is fixed by its own configuration) |
| [MiniMax Code](https://github.com/MiniMax-AI/minimax-code) | `mcode` | MiniMax Code ACP coding agent (ACP via `mcode acp`; model is managed by MCode) |
| [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) | `dsh` | DeepSeek Harness (`dsh --profile patchbay --stdio`; requires the Patchbay runtime profile to be installed; reads AGENTS.md and .dsh/skills/) |

You need at least one installed. The daemon registers each detected CLI as an available runtime.

### How It Works

1. On start, the daemon detects installed agent CLIs and registers a runtime for each agent in each watched workspace
2. The server pushes a wake signal over the WebSocket connection when work is waiting, and the daemon claims across all of its runtimes in one batch. A periodic poll (default: 30s) runs as the catch-up path — a wake signal cuts the wait short, so this interval puts no floor under normal task pickup; it bounds how long work can sit when a signal is missed or the connection is down
3. When a task arrives, it creates an isolated workspace directory, spawns the agent CLI, and streams results back
4. Heartbeats are sent periodically (default: 15s) so the server knows the daemon is alive
5. On shutdown, all runtimes are deregistered

### Configuration

Daemon behavior is configured via flags or environment variables:

| Setting | Flag | Env Variable | Default |
|---------|------|--------------|---------|
| Poll interval | `--poll-interval` | `PATCHBAY_DAEMON_POLL_INTERVAL` | `30s` (catch-up fallback; WebSocket wake signals deliver work sooner) |
| Heartbeat interval | `--heartbeat-interval` | `PATCHBAY_DAEMON_HEARTBEAT_INTERVAL` | `15s` |
| Agent timeout | `--agent-timeout` | `PATCHBAY_AGENT_TIMEOUT` | `0` (no cap; bounded by the watchdogs) |
| Agent idle watchdog | — | `PATCHBAY_AGENT_IDLE_WATCHDOG` | `2h` (`0` disables the whole watchdog suite) |
| Agent tool watchdog | — | `PATCHBAY_AGENT_TOOL_WATCHDOG` | same as the idle watchdog (`0` = never force-stop during a tool call) |
| Codex semantic inactivity timeout | `--codex-semantic-inactivity-timeout` | `PATCHBAY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT` | same as the idle watchdog (Codex's timer is not tool-aware, so it tracks the larger of the idle / tool budgets) |
| Codex first-turn no-progress timeout | — | `PATCHBAY_CODEX_FIRST_TURN_TIMEOUT` | `0` (keeps the built-in `60s` ceiling) |
| Codex handshake timeout | `--codex-handshake-timeout` | `PATCHBAY_CODEX_HANDSHAKE_TIMEOUT` | `30s`; `thread/start` and `thread/resume`: `60s` (an explicit value overrides both budgets globally) |
| OpenCode idle watchdog | — | `PATCHBAY_OPENCODE_IDLE_WATCHDOG` | `10m` (`0` falls back to the generic idle watchdog; cannot extend it) |
| Max concurrent tasks | `--max-concurrent-tasks` | `PATCHBAY_DAEMON_MAX_CONCURRENT_TASKS` | `20` |
| Daemon ID | `--daemon-id` | `PATCHBAY_DAEMON_ID` | hostname |
| Device name | `--device-name` | `PATCHBAY_DAEMON_DEVICE_NAME` | hostname |
| Runtime name | `--runtime-name` | `PATCHBAY_AGENT_RUNTIME_NAME` | `Local Agent` |
| Workspaces root | — | `PATCHBAY_WORKSPACES_ROOT` | `~/patchbay_workspaces` |
| GC enabled | — | `PATCHBAY_GC_ENABLED` | `true` (set `false`/`0` to disable) |
| GC scan interval | — | `PATCHBAY_GC_INTERVAL` | `2h` |
| GC TTL (done/cancelled issues) | — | `PATCHBAY_GC_TTL` | `24h` |
| GC completed-task TTL (issue tasks) | — | `PATCHBAY_GC_COMPLETED_TASK_TTL` | `14d` on Patchbay Cloud, `0` (disabled) elsewhere |
| GC orphan TTL (no `.gc_meta.json`) | — | `PATCHBAY_GC_ORPHAN_TTL` | `72h` |
| GC artifact TTL (completed tasks) | — | `PATCHBAY_GC_ARTIFACT_TTL` | `12h` (set `0` to disable) |
| GC artifact patterns | — | `PATCHBAY_GC_ARTIFACT_PATTERNS` | `node_modules,.next,.turbo` |
| GC repo cache TTL (`.repos`) | — | `PATCHBAY_GC_REPO_TTL` | `720h` (30d; set `0` to disable) |
| GC repo maintenance | — | `PATCHBAY_GC_REPO_MAINTENANCE_ENABLED` | `true` (set `false`/`0` to disable heavy Git maintenance only) |
| GC Hermes memory TTL (per-agent `memories/`) | — | `PATCHBAY_GC_HERMES_MEMORY_TTL` | `2160h` (90d; set `0` to disable) |
| GC Hermes session TTL (per-conversation `state.db`) | — | `PATCHBAY_GC_HERMES_SESSION_TTL` | `336h` (14d; set `0` to disable) |
| GC task temp legacy TTL (pre-lock `patchbay-task-*`) | — | `PATCHBAY_GC_TASK_TEMP_LEGACY_TTL` | `0` (disabled; set a duration to opt in) |

#### Workspace garbage collection

The daemon periodically scans `PATCHBAY_WORKSPACES_ROOT` and applies several disk-reclamation policies:

- **Full task cleanup** — when an issue's status is `done` or `cancelled` and has been idle for `PATCHBAY_GC_TTL`, the entire task directory is removed.
- **Completed-task retention bound** — `PATCHBAY_GC_COMPLETED_TASK_TTL` fully removes an inactive issue task once its `.gc_meta.json` `completed_at` age exceeds the configured duration, even while the parent issue remains open. Cleanup waits for a successful parent-issue status check, never removes an active environment, and never fully removes a `local_directory` environment. A later rerun provisions a fresh environment instead of resuming the removed checkout.
  - The default depends on where the daemon points: `14d` against Patchbay Cloud, and `0` (disabled, retain indefinitely) for self-host and every other origin — including cloud staging and previews. Set the variable to opt in or out on either side; an explicit `0` disables the policy on Cloud too.
  - Removing an environment discards work an agent left uncommitted or unpushed on its branch, along with that task's `output/` and `logs/`. The per-issue Codex session store lives outside `PATCHBAY_WORKSPACES_ROOT` under its own TTL, so a later rerun still resumes the agent's prior session — it just starts from a fresh checkout. Size the TTL against that trade, and keep it comfortably above `PATCHBAY_GC_INTERVAL`: the active-root guard protects a task that is currently running, not one whose follow-up run is queued but unclaimed.
- **Orphan cleanup** — task directories with no `.gc_meta.json` (e.g. left over from a daemon crash) are removed once they exceed `PATCHBAY_GC_ORPHAN_TTL`.
- **Artifact-only cleanup** — when a task has been completed for at least `PATCHBAY_GC_ARTIFACT_TTL` but the issue is still open, regenerable build outputs whose directory basename matches `PATCHBAY_GC_ARTIFACT_PATTERNS` are removed. The daemon also reclaims the exact managed path `codex-home/.sandbox-bin`; old task metadata without `completed_at` becomes eligible for this managed-only cleanup after its `.gc_meta.json` file has been idle for `PATCHBAY_GC_ORPHAN_TTL`. The rest of the task (source, `.git`, `output/`, `logs/`, `.gc_meta.json`, Codex auth/config/session state) is preserved so the agent can resume it.
- **Managed-cache reclamation** — the exact managed path above is reclaimed for *every* task kind once the task has been completed for `PATCHBAY_GC_ARTIFACT_TTL`, not just for issue tasks whose issue is still open. It applies even while the parent record says the directory itself must stay — an active chat session, a still-running automation run — and even when the parent record could not be reached this cycle, because the contents are regenerable and the next run re-provisions them on demand. A task currently running on the directory is never touched. Set `PATCHBAY_GC_ARTIFACT_TTL=0` to disable this along with the rest of artifact cleanup.

- **Repo cache eviction** — the bare git clones under `.repos/` are shared object stores: each task workdir is a `git worktree` off one of them rather than its own clone, so a task's `.git` is only a pointer file. They are evicted only when all of the following hold: the repo is no longer attached to any workspace this daemon watches, it has no worktrees left, and no task has created a worktree from it for `PATCHBAY_GC_REPO_TTL`. A cache created before this stamp existed is not treated as ancient — its clock starts at the first GC cycle that sees it, so upgrading does not wipe every cache. Evicting is safe by construction: the next task that needs the repo re-clones it on demand, so a wrong eviction costs a clone, not a failure.

  Short worktree cleanup and eligible cache eviction continue on every GC cycle, including while agents are active. Heavy repo maintenance (`reflog expire` and `git gc`) starts only while the daemon is otherwise idle. A checkout or newly claimed task cancels it and takes priority; interrupted work remains pending for a later idle GC cycle. Operators can disable only these heavy commands with `PATCHBAY_GC_REPO_MAINTENANCE_ENABLED=false` without disabling worktree cleanup or cache eviction.

- **Hermes session store reclamation** — a conversation's Hermes transcript (`state.db`) lives at `<profile dir>/hermes-sessions/<agent-id>/<hermes-profile>/<conversation>/`, outside any task directory, so a follow-up turn can resume it (see [Hermes agent memory](#hermes-agent-memory)). A store untouched for `PATCHBAY_GC_HERMES_SESSION_TTL` is removed. The default matches the Codex session store rather than the memory store above: these hold full transcripts, and reclaiming an idle one costs a thread that starts fresh (with a continuity notice), not an agent that forgot what it learned. A store a running task holds is never reclaimed.
- **Hermes memory store reclamation** — a Hermes agent's long-term memory (`memories/`) lives at `<profile dir>/hermes-state/<agent-id>/<hermes-profile>/`, outside any task directory, so it survives across tasks and issues (see [Hermes agent memory](#hermes-agent-memory)). A store untouched for `PATCHBAY_GC_HERMES_MEMORY_TTL` is removed, giving a deleted agent's memory an eventual-reclamation guarantee. The default is deliberately long: these are a handful of markdown files, and reclaiming one is user-visible amnesia rather than a cache miss. A store a running task holds is never reclaimed.
- **Task temp dir reclamation** — every task gets a private temp directory (`patchbay-task-*`) under the system temp base (`/tmp`, or `PATCHBAY_AGENT_TEMP_BASE`), exported to the agent as `TMPDIR`/`TMP`/`TEMP`. It is removed when the run ends, but that removal never happens when the daemon is killed and does not succeed while a file inside is still open — common on Windows, where an open handle makes the delete fail outright. These directories live outside `PATCHBAY_WORKSPACES_ROOT`, so nothing else reclaimed them and whatever the end-of-run removal missed accumulated forever. Every GC cycle now sweeps the temp base. Liveness is decided by the directory's `.task_lock` — the same OS advisory lock an env root uses, which the kernel releases when the holding process dies — not by age: a directory still in use is never removed however old it is, including one owned by a different daemon sharing the same temp base, and a directory whose owner is gone is removed on the next cycle however new it is. Directories left by a daemon predating that lock carry no lock file, so nothing can be proven about them and age is the only signal available. Reclaiming those is an operator's explicit decision: `PATCHBAY_GC_TASK_TEMP_LEGACY_TTL` defaults to `0`, which leaves them in place. Set it to a duration only once you know no pre-lock daemon is still running tasks on this machine — a task may legitimately run for weeks (there is no default agent timeout), a daemon on another profile can still be on the old binary, and every daemon on the machine shares one temp base, so a TTL here can delete a `TMPDIR` that is still in use. Each GC cycle logs how many such directories it left alone. Even with a TTL set, a directory holding no task content is never reclaimed on age — an old empty leftover, or a shell left by a daemon that died between creating the directory and publishing its lock — because holding no content is exactly what a directory currently being published looks like, and deleting one of those would take the `TMPDIR` of a task that is starting. Those shells are a few bytes each. Only entries carrying the `patchbay-task-` prefix are ever considered — the temp base itself is usually shared with other programs — and a directory this sweep cannot read is never touched.

Configured patterns are basename-only — entries containing `/` or `\` are silently dropped — and `.git` subtrees are never descended into. The managed Codex cache is matched by its exact relative path, so a repository's own `.sandbox-bin` is not removed unless an operator explicitly adds that basename to `PATCHBAY_GC_ARTIFACT_PATTERNS`. The default list (`node_modules`, `.next`, `.turbo`) is intentionally narrow; extend it per deployment if your repos consistently produce other regenerable directories (for example, `PATCHBAY_GC_ARTIFACT_PATTERNS=node_modules,.next,.turbo,target,__pycache__`). To disable artifact cleanup entirely, including the managed Codex cache, set `PATCHBAY_GC_ARTIFACT_TTL=0`.

`patchbay daemon disk-usage` reports the `.repos` footprint on its own line rather than folding it into the per-task totals — every task in a workspace checks out from that shared cache, so attributing it to individual task directories would double-count it. Note that the repo cache is reclaimed on the schedule above and not by any per-issue status change, so it is normal for it to persist after every task directory is gone.

Agent-specific overrides:

| Variable | Description |
|----------|-------------|
| `PATCHBAY_CLAUDE_PATH` | Custom path to the `claude` binary |
| `PATCHBAY_CLAUDE_MODEL` | Override the Claude model used |
| `PATCHBAY_CLAUDE_ARGS` | Default extra arguments for Claude Code runs |
| `PATCHBAY_ANTIGRAVITY_PATH` | Custom path to the `agy` binary |
| `PATCHBAY_ANTIGRAVITY_MODEL` | Override the Antigravity model used |
| `PATCHBAY_CODEBUDDY_PATH` | Custom path to the `codebuddy` binary |
| `PATCHBAY_CODEBUDDY_MODEL` | Override the CodeBuddy model used |
| `PATCHBAY_CODEBUDDY_ARGS` | Default extra arguments for CodeBuddy runs |
| `PATCHBAY_CODEARTS_PATH` | Custom path to the `codearts` launcher or binary |
| `PATCHBAY_CODEARTS_MODEL` | Override the CodeArts model used |
| `PATCHBAY_DEVECO_PATH` | Custom path to the `deveco` binary |
| `PATCHBAY_DEVECO_MODEL` | Override the DevEco Code model used |
| `PATCHBAY_CODEX_PATH` | Custom path to the `codex` binary |
| `PATCHBAY_CODEX_MODEL` | Override the Codex model used |
| `PATCHBAY_CODEX_ARGS` | Default extra arguments for Codex runs |
| `PATCHBAY_COPILOT_PATH` | Custom path to the `copilot` binary |
| `PATCHBAY_COPILOT_MODEL` | Override the Copilot model used (note: GitHub Copilot routes models through your account entitlement, so this may not be honoured) |
| `PATCHBAY_OPENCODE_PATH` | Custom path to the `opencode` binary |
| `PATCHBAY_OPENCODE_MODEL` | Override the OpenCode model used |
| `PATCHBAY_OPENCLAW_PATH` | Custom path to the `openclaw` binary |
| `PATCHBAY_OPENCLAW_MODEL` | Override the OpenClaw model used |
| `PATCHBAY_OPENCLAW_CLI_TIMEOUT` | Deadline for each `openclaw config ...` call during task preparation (default 30s; accepts `45s` or `45`). Raise it when the local CLI is slow to start; the daemon also reads it from `backends.openclaw.cli_timeout` in the CLI config |
| `PATCHBAY_HERMES_PATH` | Custom path to the `hermes` binary |
| `PATCHBAY_HERMES_MODEL` | Override the Hermes model used |
| `PATCHBAY_PI_PATH` | Custom path to the `pi` binary |
| `PATCHBAY_PI_MODEL` | Override the Pi model used |
| `PATCHBAY_CURSOR_PATH` | Custom path to the `cursor-agent` binary |
| `PATCHBAY_CURSOR_MODEL` | Override the Cursor Agent model used |
| `PATCHBAY_KIMI_PATH` | Custom path to the `kimi` binary |
| `PATCHBAY_KIMI_MODEL` | Override the Kimi model used |
| `PATCHBAY_REASONIX_PATH` | Custom path to the `reasonix` binary |
| `PATCHBAY_REASONIX_MODEL` | Override the Reasonix model used |
| `PATCHBAY_DIM_PATH` | Custom path to the `dim` binary |
| `PATCHBAY_DIM_MODEL` | Override the Dim model used |
| `PATCHBAY_KIRO_PATH` | Custom path to the `kiro-cli` binary |
| `PATCHBAY_KIRO_MODEL` | Override the Kiro model used |
| `PATCHBAY_QODER_PATH` | Custom path to the `qodercli` binary |
| `PATCHBAY_QODER_MODEL` | Override the Qoder model used |
| `PATCHBAY_QODERCLICN_PATH` | Custom path to the `qoderclicn` binary |
| `PATCHBAY_QODERCLICN_MODEL` | Override the Qoder CN model used |
| `PATCHBAY_TRAECLI_PATH` | Custom path to the `traecli` binary |
| `PATCHBAY_TRAECLI_MODEL` | Override the Trae model used (a model id from your logged-in traecli catalog, e.g. `Doubao-Seed-2.1-Pro`) |
| `PATCHBAY_GROK_PATH` | Custom path to the `grok` binary (defaults to `grok` on PATH; often `~/.grok/bin/grok`) |
| `PATCHBAY_GROK_MODEL` | Override the Grok model used (e.g. `grok-4.5`) |
| `PATCHBAY_QWEN_PATH` | Custom path to the `qwen` binary |
| `PATCHBAY_QWEN_MODEL` | Override the Qwen Code model used |
| `PATCHBAY_QWEN_ARGS` | Daemon-wide extra Qwen arguments (POSIX shellword parsing; managed protocol flags are filtered) |
| `PATCHBAY_QWENPAW_PATH` | Custom path to the `qwenpaw` binary |
| `PATCHBAY_QWENPAW_ARGS` | Daemon-wide extra QwenPaw arguments (POSIX shellword parsing; managed protocol flags are filtered) |
| `PATCHBAY_MCODE_PATH` | Custom path to the `mcode` binary |
| `PATCHBAY_DSH_PATH` | Custom path to the `dsh` binary |
| `PATCHBAY_DSH_MODEL` | Override the DeepSeek Harness model used (a model id from the dsh catalog, e.g. `deepseek-official/deepseek-chat`) |

If a previously generated `~/.patchbay/hooks` wrapper is first on `PATH` and calls the same command name again, the daemon skips that hooks directory during built-in agent discovery and records the real binary path behind it. If your interactive shell still recurses when you run `claude`, `codex`, or `hermes` manually, remove the hooks entry from your shell startup file or replace the wrapper body with an absolute `exec /path/to/real-binary "$@"`.

The daemon launches Qoder and Qoder CN as `qodercli --yolo --acp` and `qoderclicn --yolo --acp`, respectively, matching their ACP “bypass permissions” mode so tool runs do not block on interactive approval in headless runs.
The daemon launches Qwen Code as `qwen -p <prompt> --output-format stream-json`. It writes the task brief to `QWEN.md`; when an agent has managed `mcp_config`, the daemon writes a 0600 per-run JSON file and passes it through `--mcp-config <path>`, then removes it after the process exits. A null config preserves Qwen Code native MCP settings.

#### `mcp_config` on ACP runtimes

ACP-family runtimes — Hermes, Kimi, Kiro, Grok, Qoder, Reasonix, Trae, QwenPaw, MiniMax Code, Dim, and any custom runtime profile whose `protocol_family` is one of them — receive MCP servers **over the ACP session protocol**, not through a config file. The daemon translates the agent's `mcp_config` into ACP's `McpServer` array and sends it with `session/new`, and again with that runtime's resume request (`session/resume` on Hermes, Kimi, Qoder and Reasonix; `session/load` on Kiro, Grok, Trae, QwenPaw and Dim) so a resumed task keeps the same tools. MiniMax Code 0.1.2 advertises no session-loading capability, so a later run falls back to a fresh session.

Nothing is written to the runtime's own config file, and the runtime's own file is not read or merged. `~/.hermes/…`, `~/.jcode/mcp.json` and the like stay untouched; an agent's servers travel with its tasks instead of being installed per machine.

Two consequences are worth knowing before debugging a missing MCP tool:

- **`mcp_config` must use the canonical envelope**, `{"mcpServers": {"<name>": {…}}}`. Runtime-native config files that nest servers under `servers`, `mcp`, or `mcp_servers` are stored as-is but yield no servers; the daemon logs a warning naming the key it found. Entries themselves use the Claude-style shape (`command`/`args`/`env` for stdio, `url`/`headers`/`type` for remote).
- **Remote transports depend on what the runtime declares.** ACP v1 requires an omitted capability to be treated as unsupported, so `http` and `sse` entries are dropped with a warning unless the `initialize` response declares `agentCapabilities.mcpCapabilities` with that transport set to true. The built-in Hermes runtime is a verified exception: it declares no `mcpCapabilities` yet accepts both transports, so remote entries are still forwarded to it. That exception covers the Hermes binary only — a custom runtime profile with `protocol_family: hermes` runs a different implementation and keeps the standard rule. Stdio is never gated.

If a configured server produces no tools, check the daemon log for those warnings first, then confirm the runtime itself exposes the server's tools to the model — some ACP adapters apply their own tool-profile filtering after connecting.


The daemon launches QwenPaw as `qwenpaw acp --workspace <per-task dir>`. It writes the task brief to `AGENTS.md`, and materialises the run's bound skills into `<per-task dir>/skills/` plus a `skill.json` manifest, so QwenPaw discovers them through its own workspace skill discovery. `acp` and `--workspace` are reserved: `custom_args` cannot override them. QwenPaw is the one runtime with no `PATCHBAY_QWENPAW_MODEL`: its `session/set_model` writes to a shared, persistent agent config rather than the session, so Patchbay never sends it a model and leaves that choice to QwenPaw's own configuration.

The daemon launches MiniMax Code as `mcode acp`, writes the task brief to `AGENTS.md`, and injects bound skills under `.minimax/skills/`. MCode owns model selection and currently advertises `loadSession: false`; Patchbay therefore starts a fresh MCode session when a later run cannot load the saved session.

#### Hermes agent memory

Hermes discovers skills only from its own home, so binding Patchbay skills to a Hermes agent makes the daemon build a per-task `HERMES_HOME` overlay for that agent. The agent's long-term memory (`memories/`) does **not** live inside that task-scoped overlay: it is linked to a persistent store at

```
<profile dir>/hermes-state/<agent-id>/<hermes-profile>/
```

so the same agent keeps its memory across tasks and issues. `<hermes-profile>` is the profile the agent resolves to (`default`, a named profile from `-p/--profile` or `active_profile`, or a hash for an out-of-tree custom `HERMES_HOME`) — pointing an agent at a different profile gives it a different memory line, matching Hermes' own "a profile is an isolated instance" model.

Consequences worth knowing:

- **Memory is agent-scoped but runtime-local.** One agent's memory is never visible to another, and the user's own `~/.hermes/memories` is never read or written. The store lives in this runtime's Patchbay profile directory, so it does **not** follow the agent to another machine — an agent that runs on two runtimes has a separate memory line on each. Everything else in the home — auth, config, plugins — is still shared from the user's real home by symlink, so the agent does not need its own login.
- **To carry existing local memory in**, copy it into the store once: `cp -R ~/.hermes/memories/. "<profile dir>/hermes-state/<agent-id>/default/"`. To wipe an agent's memory, delete that directory.
- **Conversation history is covered too, in a separate store.** Hermes keeps every ACP session in `<HERMES_HOME>/state.db`, which the overlay links to a per-conversation store at `<profile dir>/hermes-sessions/<agent-id>/<hermes-profile>/<issue-id | chat_\<chat-session-id\>>/`, so a follow-up turn resumes the actual transcript. The shard is per conversation rather than per agent on purpose: tasks of one conversation run one after another, so a shard has a single writer at a time, while two issues never share a database. A host that cannot create the link (Windows without symlink privileges) keeps the database task-local instead, untouched — the link is proven creatable before anything is moved, and a copy is never used, because a copied SQLite database would absorb the turn's writes into a file the next task discards.
- **Concurrent tasks of one agent are last-writer-wins.** Hermes rewrites its memory files whole, so two tasks writing memory at the same time can overwrite each other.
- **Every Hermes agent gets the overlay in practice**, so every one of them gets a persistent memory store. The daemon builds the overlay only when a task carries skills, but the server appends Patchbay's built-in skills to every agent's skill set (`LoadAgentSkillBundles`), so that list is never empty — leaving an agent's own skill list empty does not opt out of the overlay, and is not a way to keep using the host's `~/.hermes/memories`.

`PATCHBAY_CLAUDE_ARGS`, `PATCHBAY_CODEX_ARGS`, `PATCHBAY_CODEBUDDY_ARGS`, `PATCHBAY_QWEN_ARGS`, and `PATCHBAY_QWENPAW_ARGS` are parsed with POSIX shellword quoting, so values such as `--model "gpt-5.1 codex" --sandbox read-only` are split like a shell command line. Agent arguments are applied in this order: hardcoded Patchbay defaults, daemon-wide env defaults, then per-agent `custom_args` from the task.

### Self-Hosted Server

When connecting to a self-hosted Patchbay instance, the easiest approach is:

```bash
# One command — configures for localhost, authenticates, starts daemon
patchbay setup self-host

# Or for on-premise with custom domains:
patchbay setup self-host --server-url https://api.example.com --app-url https://app.example.com
```

Or configure manually:

```bash
# Set URLs individually
patchbay config set server_url http://localhost:8080
patchbay config set app_url http://localhost:3000

# For production with TLS:
# patchbay config set server_url https://api.example.com
# patchbay config set app_url https://app.example.com

patchbay login
patchbay daemon start
```

### Profiles

Profiles let you run multiple daemons on the same machine — for example, one for production and one for a staging server.

```bash
# Set up a staging profile
patchbay setup self-host --profile staging --server-url https://api-staging.example.com --app-url https://staging.example.com

# Start its daemon
patchbay daemon start --profile staging

# Default profile runs separately
patchbay daemon start
```

Each profile gets its own config directory (`~/.patchbay/profiles/<name>/`), daemon state, health port, and workspace root. Daemon state means that profile's own `daemon.log`, `daemon.err.log`, and `daemon.pid` live in that directory too — see [Start](#start) for the layout, and pass `--profile <name>` to `daemon status` / `daemon logs` to act on it.

## Workspaces

### Working with multiple workspaces

Every command runs against a single workspace. The CLI resolves which one in this order (highest priority first):

1. `--workspace-id <id>` flag on the command
2. `PATCHBAY_WORKSPACE_ID` environment variable
3. The default workspace stored in your current profile (set by `patchbay workspace switch` or `patchbay login`)

`patchbay workspace switch <id|slug>` is the day-to-day way to change the default workspace. For scripting and headless setups where you don't want any stored state, prefer the `--workspace-id` flag or the env variable. `patchbay config set workspace_id <id>` is the low-level equivalent of `switch` (it writes the same setting but skips the access check).

If you need full isolation between organizations or accounts — separate tokens, separate daemons, separate config dirs — use `--profile <name>` instead. Each profile keeps its own default workspace.

### List Workspaces

```bash
patchbay workspace list
patchbay workspace list --full-id
patchbay workspace list --output json
```

The current default workspace is marked with `*`. Table output shows short UUID prefixes — pass `--full-id` when you need the canonical UUIDs.

### Switch Default Workspace

```bash
patchbay workspace switch <workspace-id>
patchbay workspace switch <slug>
```

Verifies you have access to the workspace, then sets it as the default for the current profile. Subsequent commands without `--workspace-id` and `PATCHBAY_WORKSPACE_ID` target this workspace. Pair `--profile` if you want to change a non-default profile's workspace.

### Get Details

```bash
patchbay workspace get <workspace-id>
patchbay workspace get <workspace-id> --output json
```

Passing no `<workspace-id>` resolves to the current default workspace, so `patchbay workspace get` doubles as "what workspace am I on?".

### List Members

```bash
patchbay workspace member list <workspace-id>
```

## Issues

### List Issues

```bash
patchbay issue list
patchbay issue list --status in_progress
patchbay issue list --priority urgent --executor "Agent Name"
patchbay issue list --executor-id 5fb87ac7-23b5-4a7a-81fa-ed295a54545d
patchbay issue list --full-id
patchbay issue list --limit 20 --output json
patchbay issue list --status todo --sort position       # board order (the default)
patchbay issue list --sort created_at --direction desc  # newest first
```

Table output shows a routable issue `KEY` such as `MUL-123`; copy that key into follow-up commands like `issue get`, `issue comment list`, `issue status`, or `--parent`. Add `--full-id` when you need canonical UUIDs. Available filters: `--status`, `--priority`, `--owner` / `--owner-id`, `--executor` / `--executor-id`, `--project`, `--metadata`, `--limit`. An owner is a workspace member; an executor is an agent or team. Use the role-specific ID flags for unambiguous filtering when names overlap.

Results come back in board order (`position`, ascending) by default. Pass `--sort` to change the column (`position`, `title`, `created_at`, `start_date`, `due_date`, `priority`) and `--direction asc|desc` to flip the order. `position` is always ascending (it is the manual drag order), so `--direction` is rejected when `--sort` is `position` or omitted — use it only with `title`, `created_at`, `start_date`, `due_date`, or `priority`.

Use `--metadata key=value` (repeatable; combined with AND) to filter by per-issue metadata. The value is JSON-parsed: `true`/`false` become bool, numbers become numbers, anything else is a string. Wrap as `'"42"'` to force a string when the value would otherwise sniff as a number:

```bash
patchbay issue list --metadata pipeline_status=waiting_review
patchbay issue list --metadata pr_number=482 --metadata is_blocked=true
```

### Get Issue

```bash
patchbay issue get <id>
patchbay issue get <id> --output json
```

### Create Issue

```bash
patchbay issue create --title "Fix login bug" --description "..." --priority high --executor "Lambda"
patchbay issue create --title "Fix login bug" --executor-id 5fb87ac7-23b5-4a7a-81fa-ed295a54545d
```

Flags: `--title` (required), `--description`, `--status`, `--priority`, `--owner` / `--owner-id`, `--executor` / `--executor-id`, `--reviewer` / `--reviewer-id`, `--parent`, `--project`, `--due-date`. The owner must be a workspace member; the executor must be an agent or team; the reviewer may be a member, agent, or team. Use role-specific ID flags when scripting against the IDs returned by `patchbay workspace member list --output json` / `patchbay agent list --output json`.

### Update Issue

```bash
patchbay issue update <id> --title "New title" --priority urgent
patchbay issue update <id> --position 4.5
```

`--position` sets the raw ordering value within the board column (lower sorts first). For relative moves, `issue reorder` is easier because it works out the value for you.

### Reorder Issue

Move an issue within its current status column. The new ordering value is computed the same way the board's drag-and-drop computes it, so the CLI and UI agree on where the issue lands.

```bash
patchbay issue reorder <id> --top              # top of its status column
patchbay issue reorder <id> --bottom           # bottom of its status column
patchbay issue reorder <id> --before <other>   # directly above another issue in the same column
patchbay issue reorder <id> --after  <other>   # directly below another issue in the same column
```

Pick exactly one of `--top`, `--bottom`, `--before`, or `--after`. Reorder stays inside the issue's current column, so `--before` / `--after` must name an issue in that same column. To move an issue to a different column, change its status first with `issue status`, then reorder within the new column.

### Assign Issue

```bash
patchbay issue assign <id> --to "Lambda"
patchbay issue assign <id> --to-id 5fb87ac7-23b5-4a7a-81fa-ed295a54545d
patchbay issue assign <id> --unassign
```

Pass `--to-id <uuid>` to assign by canonical UUID (mutually exclusive with `--to`); useful when names overlap across members and agents.

### Change Status

```bash
patchbay issue status <id> in_progress
```

Valid statuses: `backlog`, `todo`, `in_progress`, `in_review`, `done`, `blocked`, `cancelled`.

### Comments

```bash
# List comments — flat timeline, chronological. Hard cap of 2000 rows; on
# long-running issues prefer one of the thread-aware reads below to keep
# context windows tight.
patchbay issue comment list <issue-id>

# Single thread (root + every descendant). Anchor may be the root itself
# or any reply inside the thread — the server walks up to the root.
patchbay issue comment list <issue-id> --thread <comment-id>

# Single thread, capped to the N most recent replies. The thread root is
# always included (even with --tail 0), so an agent landing on a long
# thread keeps the "what is this about" context without dragging hundreds
# of replies into its prompt.
patchbay issue comment list <issue-id> --thread <comment-id> --tail 30

# Scroll older replies inside the same thread. --before / --before-id are
# the reply cursor that the previous response emitted on stderr as
# `Next reply cursor: --before <ts> --before-id <reply-id>`.
patchbay issue comment list <issue-id> --thread <comment-id> --tail 30 \
    --before <ts> --before-id <reply-id>

# Most recently active threads (root + every descendant), grouped by
# thread. Returns N complete conversational arcs, oldest-active first so
# the freshest thread sits closest to "now" in an agent prompt.
patchbay issue comment list <issue-id> --recent 10

# Scroll older threads. Under --recent, --before / --before-id are a
# THREAD cursor (thread last_activity_at + root id), emitted on stderr as
# `Next thread cursor: --before <ts> --before-id <root-id>`.
patchbay issue comment list <issue-id> --recent 10 \
    --before <ts> --before-id <root-id>

# Incremental polling. Combines with --thread or --recent; filters out
# replies created on or before <ts> from the page (the thread root is
# exempt so the agent always gets context).
patchbay issue comment list <issue-id> --thread <comment-id> --tail 30 \
    --since <RFC3339-timestamp>

# Add a comment
patchbay issue comment add <issue-id> --content "Looks good, merging now"

# Reply to a specific comment
patchbay issue comment add <issue-id> --parent <comment-id> --content "Thanks!"

# Delete a comment
patchbay issue comment delete <comment-id>
```

**`--before` / `--before-id` semantics depend on the paging mode**, by
design — same flag, different scope:

| Mode | What the cursor walks | stderr label |
| --- | --- | --- |
| `--recent N` | Older *threads* (last_activity_at, root_id) | `Next thread cursor` |
| `--thread <id> --tail N` | Older *replies* inside that thread (created_at, id) | `Next reply cursor` |

Outside those two modes (`--thread` without `--tail`, or no `--thread`
and no `--recent`) the cursor flags are rejected so they cannot silently
no-op. The server emits the cursor headers (`X-Patchbay-Next-Before` /
`X-Patchbay-Next-Before-Id`) only when an older page actually exists —
exact-boundary pages (e.g. `--tail 3` on a thread with exactly 3
replies) intentionally return no cursor so callers stop paginating.

When `--since` is combined with `--recent` or `--thread --tail`, the
server additionally suppresses the cursor once the cursor target itself
is older than `since`. Older pages walk strictly older rows, so they
cannot satisfy `> since` either — emitting a cursor there would just
hand back root-only pages until the caller reaches the start of the
thread / issue. Incremental polling stops at the first page whose
cursor target falls before the watermark.

### Metadata

Per-issue metadata is a small KV map agents use to track pipeline state (PR number, pipeline status, waiting_on, ...). Keys match `^[a-zA-Z_][a-zA-Z0-9_.-]{0,63}$`, values are primitives (string / number / bool), max 50 keys per issue, blob capped at 8KB.

The bar for writing is high: pin a value only when it is materially important to the issue AND likely to be re-read by future runs on this same issue (the PR URL, the deploy URL, what we're blocked on). Most runs write zero new keys — that's the expected case. Don't pin runtime bookkeeping like `attempts`, single-run investigation notes, large logs, secrets/tokens, or description/comment copies — see the agent runtime prompt for the full anti-pattern list.

```bash
# List every key on an issue
patchbay issue metadata list <issue-id>

# Read a single key
patchbay issue metadata get <issue-id> --key pipeline_status

# Write a single key — value auto-typed (true/false → bool, numbers → number, else string)
patchbay issue metadata set <issue-id> --key pipeline_status --value waiting_review
patchbay issue metadata set <issue-id> --key pr_number --value 482
patchbay issue metadata set <issue-id> --key is_blocked --value true

# Force a specific type when sniffing would pick the wrong one
patchbay issue metadata set <issue-id> --key code --value 42 --type string

# Remove a key
patchbay issue metadata delete <issue-id> --key pipeline_status
```

All writes are single-key atomic — concurrent agents writing different keys do not lose each other's updates. To query, use `patchbay issue list --metadata key=value` (see *List Issues* above).

### Subscribers

```bash
# List subscribers of an issue
patchbay issue subscriber list <issue-id>

# Subscribe yourself to an issue
patchbay issue subscriber add <issue-id>

# Subscribe another member or agent by name
patchbay issue subscriber add <issue-id> --user "Lambda"

# Unsubscribe yourself
patchbay issue subscriber remove <issue-id>

# Unsubscribe another member or agent
patchbay issue subscriber remove <issue-id> --user "Lambda"
```

Subscribers receive notifications about issue activity (new comments, status changes, etc.). Without `--user`, the command acts on the caller.

### Execution History

```bash
# List all execution runs for an issue
patchbay issue runs <issue-id>
patchbay issue runs <issue-id> --full-id
patchbay issue runs <issue-id> --output json

# View messages for a specific execution run
patchbay issue run-messages <task-id>
patchbay issue run-messages <short-task-id> --issue <issue-id>
patchbay issue run-messages <task-id> --output json

# Incremental fetch (only messages after a given sequence number)
patchbay issue run-messages <task-id> --since 42 --output json

# Aggregated token usage for an issue (sum across all its task runs)
patchbay issue usage <issue-id>
patchbay issue usage <issue-id> --output json
```

The `usage` command returns the aggregated token usage for an issue, summed across all of its task runs: input tokens, output tokens, cache read/write tokens, and the run count (`task_count`). It wraps `GET /api/issues/<id>/usage` — the same figures the issue detail view shows. Use `--output json` to feed billing/cost tooling.

The `runs` command shows all past and current executions for an issue, including running tasks. Table output uses short task UUID prefixes by default; pass `--full-id` to print canonical task UUIDs. The `run-messages` command accepts full task UUIDs directly; copied short task prefixes must be scoped with `--issue <issue-id>` so the CLI only checks that issue's runs. It shows the detailed message log (tool calls, thinking, text, errors) for a single run. Use `--since` for efficient polling of in-progress runs.

## Projects

Projects group related issues (e.g. a sprint, an epic, a workstream). Every project
belongs to a workspace and can optionally have a lead (member or agent).

### List Projects

```bash
patchbay project list
patchbay project list --status in_progress
patchbay project list --output json
```

Available filters: `--status`.

### Get Project

```bash
patchbay project get <id>
patchbay project get <id> --output json
```

### Create Project

```bash
patchbay project create --title "2026 Week 16 Sprint" --icon "🏃" --lead "Lambda"
```

Flags: `--title` (required), `--description`, `--status`, `--icon`, `--lead`, `--start-date`, `--due-date`. Dates are calendar days (`YYYY-MM-DD`).

### Update Project

```bash
patchbay project update <id> --title "New title" --status in_progress
patchbay project update <id> --lead "Lambda"
patchbay project update <id> --due-date 2026-04-15
```

Flags: `--title`, `--description`, `--status`, `--icon`, `--lead`, `--start-date`, `--due-date`. For the date flags, pass an empty string (e.g. `--start-date ""`) to clear the date.

### Change Status

```bash
patchbay project status <id> in_progress
```

Valid statuses: `planned`, `in_progress`, `paused`, `completed`, `cancelled`.

### Delete Project

```bash
patchbay project delete <id>
```

### Associating Issues with Projects

Use the `--project` flag on `issue create` / `issue update` to attach an issue to a
project, or on `issue list` to filter issues by project:

```bash
patchbay issue create --title "Login bug" --project <project-id>
patchbay issue update <issue-id> --project <project-id>
patchbay issue list --project <project-id>
```

## Setup

```bash
# One-command setup for Patchbay Cloud: configure, authenticate, and start the daemon
patchbay setup

# For local self-hosted deployments
patchbay setup self-host

# Custom ports
patchbay setup self-host --port 9090 --frontend-port 4000

# On-premise with custom domains
patchbay setup self-host --server-url https://api.example.com --app-url https://app.example.com
```

`patchbay setup` configures the CLI, opens your browser for authentication, and starts the daemon — all in one step. Use `patchbay setup self-host` to connect to a self-hosted server instead of Patchbay Cloud.

## Configuration

### View Config

```bash
patchbay config show
```

Shows config file path, server URL, app URL, and default workspace.

### Set Values

```bash
patchbay config set server_url https://api.example.com
patchbay config set app_url https://app.example.com
patchbay config set workspace_id <workspace-id>
```

`config set workspace_id <id>` is the low-level interface — it writes the value verbatim without checking that the workspace exists or that you have access. Prefer `patchbay workspace switch <id|slug>` for day-to-day workspace changes; it does both checks before saving.

## Automation Commands

Automations are scheduled/triggered automations that dispatch agent tasks (either by creating an issue or by running an agent directly).

### List Automations

```bash
patchbay automation list
patchbay automation list --full-id
patchbay automation list --status active --output json
```

Automation table IDs are short UUID prefixes; follow-up automation commands accept copied prefixes when they are unique in the current workspace. Use `--full-id` to print canonical UUIDs.

### Get Automation Details

```bash
patchbay automation get <id>
patchbay automation get <id> --output json   # includes triggers
```

In JSON output `triggers` is a **top-level key alongside `automation`**, not nested
inside it — the payload is `{"automation": {...}, "triggers": [...], "collaborators": [...]}`.
Read trigger ids with `jq '.triggers[].id'`, or use `automation trigger-list` below.
The table output shows only the automation's own fields, not its triggers.

### Create / Update / Delete

```bash
patchbay automation create \
  --title "Nightly bug triage" \
  --description "Scan todo issues and prioritize." \
  --agent "Lambda" \
  --mode create_issue \
  --subscriber "Alice"

patchbay automation update <id> --status paused
patchbay automation update <id> --description "New prompt"
patchbay automation update <id> --subscriber "Alice" --subscriber "Bob"
patchbay automation update <id> --clear-subscribers
patchbay automation delete <id>
```

`--mode` accepts `create_issue` (creates a new issue on each run and assigns it to the agent) or `run_only` (enqueues a direct agent task without creating an issue). `--agent` accepts either a name or UUID.
`--subscriber` accepts a workspace member name or user ID and may be repeated; on update it replaces the automation's subscriber template. Subscribers receive inbox notifications for issues created by a `create_issue` automation. Use `--clear-subscribers` to remove all automation subscribers.

### Manual Trigger

```bash
patchbay automation trigger <id>            # Fires the automation once, returns the run
```

### Run History

```bash
patchbay automation runs <id>
patchbay automation runs <id> --limit 50 --output json
```

### Schedule Triggers

```bash
patchbay automation trigger-list <automation-id>              # ids, kind, schedule, next run
patchbay automation trigger-list <automation-id> --full-id    # canonical UUIDs
patchbay automation trigger-add <automation-id> --cron "0 9 * * 1-5" --timezone "America/New_York"
patchbay automation trigger-update <automation-id> <trigger-id> --enabled=false
patchbay automation trigger-delete <automation-id> <trigger-id>
```

`trigger-list` is the way to obtain the `<trigger-id>` that `trigger-update`,
`trigger-delete` and `trigger-rotate-url` require. Like automation ids, trigger ids
may be passed as a short prefix as long as it is unique within that automation; use
`--full-id` to print canonical UUIDs. Webhook credentials are redacted in this
output — use `automation get <id> --output json --show-secrets` to reveal them.

The CLI exposes cron-based `schedule` triggers via `trigger-add`, and `webhook`
triggers via `trigger-add --kind webhook` plus `trigger-rotate-url`. The data model
also defines an `api` kind, which is not surfaced here.

## Other Commands

```bash
patchbay version              # Show CLI version and commit hash
patchbay update               # Update to latest version
patchbay agent list           # List agents in the current workspace
```

## Output Formats

Most commands support `--output` with two formats:

- `table` — human-readable table (default for list commands)
- `json` — structured JSON (useful for scripting and automation)

```bash
patchbay issue list --output json
patchbay daemon status --output json
```

## Error Messages

The CLI funnels command errors returned to the top-level handler through a
single user-facing translation layer (`server/internal/cli/errors.go`) so that
what you see on the terminal is a short, actionable sentence rather than a raw
Go error, an HTTP status line, or an internal `resolve issue: ...` chain. (A
few commands print their own output or run deliberate fast probes — for example
`setup`'s short `/health` reachability check — and don't go through this
layer.) The underlying detail is still available on demand (see `--debug`).

### What you see

- **Friendly, single-line message.** Transport failures (timeout, DNS,
  connection refused, TLS) and HTTP status failures (401/403/404/409/400·422/
  429/5xx) are each rendered as one clear sentence with a next step — for
  example a timeout suggests checking the network or raising
  `PATCHBAY_HTTP_TIMEOUT`, and a 401 tells you to run `patchbay login`.
- **Server-provided validation messages are preserved.** For a 400/422 that
  carries a message from the server, that message is shown verbatim
  (`Invalid request: <server message>`); only when there is none do you get the
  generic "check your values / run with --help" hint.
- **No leaked internals by default.** Raw URLs, status lines, JSON bodies, and
  the internal verb chain are hidden unless you ask for them.

### Language

Messages default to **English**, matching the rest of the CLI's help output.
If a Chinese locale is detected in `LC_ALL`, `LC_MESSAGES`, or `LANG` (in that
precedence order), messages switch to **Chinese**. No flag is needed; set the
locale as usual:

```bash
LANG=zh_CN.UTF-8 patchbay issue get MUL-9999   # 错误信息显示为中文
```

### Exit codes

The process exit code is tiered so scripts can branch on the failure class:

| Exit code | Meaning |
| --- | --- |
| `0` | success |
| `1` | generic / unclassified error |
| `2` | network error (timeout, DNS, connection refused, TLS, offline) |
| `3` | authentication / authorization (HTTP 401, 403) |
| `4` | not found (HTTP 404) |
| `5` | validation (HTTP 400, 422) |

```bash
patchbay issue get MUL-9999
if [ $? -eq 4 ]; then echo "no such issue"; fi
```

### Seeing the full detail (`--debug`)

Pass the global `--debug` flag (or set `PATCHBAY_DEBUG=1`) to print the complete
original error chain — the internal verb chain, the request method/path/status,
and the raw server body — underneath the friendly message. Use it when you need
to file a bug or understand exactly what the server returned:

```bash
patchbay issue list --debug
PATCHBAY_DEBUG=1 patchbay issue update MUL-1234 --title "x"
```

### Request timeout

API requests use a default timeout of 30 seconds. Override it with
`PATCHBAY_HTTP_TIMEOUT` when you are on a slow network; it accepts a Go duration
(`45s`, `2m`) or a plain number of seconds (`45`). Command-level deadlines are
always at least this value, so raising it takes effect across all commands.

```bash
PATCHBAY_HTTP_TIMEOUT=60s patchbay issue list
```

### Stall detection (skill commands)

A total-elapsed timeout punishes the transfer that is working: a large skill
arriving steadily over a slow link is cut off mid-body, while a dead connection
is held open for the full budget. The `skill` commands therefore fail on a lack
of *progress* instead:

- A read that receives no bytes for **15 seconds** fails immediately, reported
  as a stalled transfer rather than a timeout.
- A transfer that keeps producing bytes runs to completion, however long it
  takes, behind a loose **10 minute** whole-request ceiling.

Override the no-progress budget with `PATCHBAY_HTTP_STALL_TIMEOUT` (same format
as `PATCHBAY_HTTP_TIMEOUT`). If only `PATCHBAY_HTTP_TIMEOUT` is set it applies on
this path too, as the no-progress budget — it keeps meaning "the longest I will
wait for this server", not "the longest this download may take".

```bash
PATCHBAY_HTTP_STALL_TIMEOUT=45s patchbay skill get <id>
```

Every other command still uses the total-elapsed timeout above. Stall detection
starts here because skill payloads are the largest responses the CLI reads; the
mechanism itself is not skill-specific.

### Skill payload size

`patchbay skill get` and `patchbay skill files list` return **metadata only** by
default — path, byte size and content hash for each file, plus the size and
hash of the SKILL.md body. Sizes are what tell you which file makes a skill
large, and they stay available no matter how large it gets.

Pass `--with-content` when you actually need the bodies:

```bash
patchbay skill files list <id>                  # paths and sizes
patchbay skill files list <id> --with-content   # bodies inlined
```

On the API, both endpoints accept `?include=content` and `?include=metadata`.
A request that sends neither still gets `content`, on both endpoints, so a
server upgrade never changes what an un-upgraded client receives — it is the
CLI that asks for the smaller shape.
