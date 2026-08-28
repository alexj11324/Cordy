# Patchbay CLI — Installation Guide for AI Agents

> **Audience:** This document is designed for AI agents (Claude Code, Codex, etc.) to read and execute step-by-step. Each step includes the exact shell commands to run, expected outcomes, and error handling.

## One-Line Instruction for Users

Give this instruction to your AI agent:

```
Fetch https://github.com/alexj11324/Cordy/blob/main/CLI_INSTALL.md and follow the instructions to install Patchbay CLI, log in, and start the daemon on this machine.
```

---

## Step 1: Check if Patchbay CLI is already installed

Run:

```bash
patchbay version
```

- **If it prints a version string** (e.g. `patchbay v0.x.x`): skip to **Step 3**.
- **If command not found**: continue to **Step 2**.

---

## Step 2: Install the Patchbay CLI

> **Windows users:** Skip to [Option B: Windows (PowerShell)](#option-b-windows-powershell) below.

### Option A: Download from GitHub Releases (macOS/Linux)

Download the binary directly from the canonical repository.

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
curl -sL "https://github.com/alexj11324/Cordy/releases/download/${LATEST}/patchbay-cli-${VERSION}-${OS}-${ARCH}.tar.gz" -o /tmp/patchbay.tar.gz
tar -xzf /tmp/patchbay.tar.gz -C /tmp patchbay
sudo mv /tmp/patchbay /usr/local/bin/patchbay
rm /tmp/patchbay.tar.gz
```

Verify:

```bash
patchbay version
```

**If this fails:**
- Check that `/usr/local/bin` is in `$PATH`.
- On Linux, you may need `chmod +x /usr/local/bin/patchbay`.
- If `sudo` is not available, install to a user-writable directory: `mv /tmp/patchbay ~/.local/bin/patchbay` and ensure `~/.local/bin` is in `$PATH`.

### Option B: Windows (PowerShell)

Run in PowerShell (no admin required):

```powershell
irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex
```

This downloads the latest Windows binary from GitHub Releases, installs it to `%USERPROFILE%\.patchbay\bin\`, and adds it to your user PATH.

Verify:

```powershell
patchbay version
```

**If this fails:**
- Restart your terminal so the updated PATH takes effect.
- If your execution policy blocks the script: `Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned` then re-run.

---

## Step 3: Log in

Run:

```bash
patchbay login
```

**Important:** This command opens a browser window for OAuth authentication. Tell the user:

> "A browser window will open for Patchbay login. Please complete the authentication in your browser, then come back here."

Wait for the command to complete. It will automatically discover and watch all workspaces the user belongs to.

Verify:

```bash
patchbay auth status
```

Expected output should show the authenticated user and server URL.

**If login fails:**
- If no browser is available (headless environment), the user can generate a Personal Access Token at `https://aspectlylabs.com/settings?tab=tokens` and run: `patchbay login --token <pby_...>` (use `--token=` with an empty value to be prompted interactively).
- If the server URL needs to be customized: `patchbay config set server_url <url>` before logging in.

---

## Step 4: Start the daemon

First, check if the daemon is already running:

```bash
patchbay daemon status
```

- **If status is "running"**: skip to **Step 5**.
- **If status is "stopped"**: start it:

```bash
patchbay daemon start
```

Wait 3 seconds, then verify:

```bash
patchbay daemon status
```

Expected output should show `running` status with detected agents (e.g. `claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, `grok`).

**If daemon fails to start:**
- Check logs: `patchbay daemon logs`
- If a port conflict occurs, the daemon may already be running under a different profile.
- If no agents are detected, ensure at least one AI CLI (`claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, or `grok`) is installed and on the `$PATH`.

---

## Step 5: Verify everything is working

Run:

```bash
patchbay daemon status
```

Confirm:
1. Status is `running`
2. At least one agent is listed (e.g. `claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, or `grok`)
3. At least one workspace is being watched

If the agents list is empty, tell the user:

> "The Patchbay daemon is running but no AI agent CLIs were detected. Please install at least one supported CLI (`claude`, `codex`, `copilot`, `opencode`, `openclaw`, `hermes`, `pi`, `cursor-agent`, or `grok`), then restart the daemon with `patchbay daemon stop && patchbay daemon start`."

---

## Summary

When all steps are complete, inform the user:

> "Patchbay CLI is installed and the daemon is running. Agents in your workspaces can now execute tasks on this machine. You can manage workspaces with `patchbay workspace list` and view daemon logs with `patchbay daemon logs -f`."
