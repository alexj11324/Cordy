# Orvilo CLI — Installation Guide for AI Agents

> **Audience:** This document is designed for AI agents (Claude Code, Codex, etc.) to read and execute step-by-step. Each step includes the exact shell commands to run, expected outcomes, and error handling.

## One-Line Instruction for Users

Give this instruction to your AI agent:

```
Fetch https://github.com/alexj11324/Cordy/blob/main/CLI_INSTALL.md and follow the instructions to install Orvilo CLI, log in, and start the daemon on this machine.
```

---

## Step 1: Check if Orvilo CLI is already installed

Run:

```bash
orvilo version
```

- **If it prints a version string** (e.g. `orvilo v0.x.x`): skip to **Step 3**.
- **If command not found**: continue to **Step 2**.

---

## Step 2: Install the Orvilo CLI

> **Windows users:** Skip to [Option C: Windows (PowerShell)](#option-c-windows-powershell) below.

### Option A: Homebrew (preferred — macOS/Linux)

Check if Homebrew is available:

```bash
which brew
```

If `brew` is found, install via Homebrew:

```bash
brew install alexj11324/tap/orvilo
```

Then verify:

```bash
orvilo version
```

If the version prints successfully, skip to **Step 3**.

To upgrade later, run:

```bash
brew upgrade alexj11324/tap/orvilo
```

### Option B: Download from GitHub Releases (macOS/Linux, no Homebrew)

If Homebrew is not available, download the binary directly.

Detect OS and architecture, then download the correct archive:

```bash
OS=$(uname -s | tr '[:upper:]' '[:lower:]')   # "darwin" or "linux"
ARCH=$(uname -m)                                # "x86_64" or "arm64"

# Normalize architecture name
if [ "$ARCH" = "x86_64" ]; then
  ARCH="amd64"
fi

# Get the latest release tag from GitHub
LATEST=$(curl -sI https://github.com/alexj11324/Cordy/releases/latest | grep -i '^location:' | sed 's/.*tag\///' | tr -d '\r\n')

# Download and extract
VERSION="${LATEST#v}"
curl -sL "https://github.com/alexj11324/Cordy/releases/download/${LATEST}/orvilo-cli-${VERSION}-${OS}-${ARCH}.tar.gz" -o /tmp/orvilo.tar.gz
tar -xzf /tmp/orvilo.tar.gz -C /tmp orvilo
sudo mv /tmp/orvilo /usr/local/bin/orvilo
rm /tmp/orvilo.tar.gz
```

Verify:

```bash
orvilo version
```

**If this fails:**
- Check that `/usr/local/bin` is in `$PATH`.
- On Linux, you may need `chmod +x /usr/local/bin/orvilo`.
- If `sudo` is not available, install to a user-writable directory: `mv /tmp/orvilo ~/.local/bin/orvilo` and ensure `~/.local/bin` is in `$PATH`.

### Option C: Windows (PowerShell)

Run in PowerShell (no admin required):

```powershell
irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex
```

This downloads the latest Windows binary from GitHub Releases, installs it to `%USERPROFILE%\.orvilo\bin\`, and adds it to your user PATH.

Verify:

```powershell
orvilo version
```

**If this fails:**
- Restart your terminal so the updated PATH takes effect.
- The Windows installer downloads the release binary directly; Scoop is not required.
- If your execution policy blocks the script: `Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned` then re-run.

---

## Step 3: Log in

Run:

```bash
orvilo login
```

This command uses device authorization and does not need a browser on the CLI machine. Tell the user:

> "Orvilo will print a verification URL and one-time code. Open the URL on a computer where you are signed in, enter the code, and approve access."

Wait for the command to complete after approval. It will automatically discover and watch all workspaces the user belongs to.

Verify:

```bash
orvilo auth status
```

Expected output should show the authenticated user and server URL.

**If login fails:**
- On a headless machine, open the printed verification URL on another signed-in computer and enter the one-time code there. No browser or callback listener is required on the CLI machine.
- If the user already has a personal access token, run `orvilo login --token=` and paste it when prompted. Do not put the token in shell history or documentation.
- If the server URL needs to be customized: `orvilo config set server_url <url>` before logging in.

---

## Step 4: Start the daemon

First, check if the daemon is already running:

```bash
orvilo daemon status
```

- **If status is "running"**: skip to **Step 5**.
- **If status is "stopped"**: start it:

```bash
orvilo daemon start
```

Wait 3 seconds, then verify:

```bash
orvilo daemon status
```

Expected output should show `running` status with detected agents (e.g. `claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, `grok`).

**If daemon fails to start:**
- Check logs: `orvilo daemon logs`
- If a port conflict occurs, the daemon may already be running under a different profile.
- If no agents are detected, ensure at least one AI CLI (`claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, or `grok`) is installed and on the `$PATH`.

---

## Step 5: Verify everything is working

Run:

```bash
orvilo daemon status
```

Confirm:
1. Status is `running`
2. At least one agent is listed (e.g. `claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, or `grok`)
3. At least one workspace is being watched

If the agents list is empty, tell the user:

> "The Orvilo daemon is running but no AI agent CLIs were detected. Please install at least one supported CLI (`claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, or `grok`), then restart the daemon with `orvilo daemon stop && orvilo daemon start`."

---

## Summary

When all steps are complete, inform the user:

> "Orvilo CLI is installed and the daemon is running. Agents in your workspaces can now execute tasks on this machine. You can manage workspaces with `orvilo workspace list` and view daemon logs with `orvilo daemon logs -f`."
