import { spawnSync } from "node:child_process";
import { mkdir, open, readFile, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

function commandOutput(result, command, absentStatus) {
  if (result.error) throw result.error;
  if (result.status === absentStatus) return null;
  if (result.status !== 0) {
    throw new Error(`${command} failed with exit code ${result.status}`);
  }
  const output = String(result.stdout || "").trim();
  if (!output) {
    throw new Error(`${command} returned no process start identity`);
  }
  return output;
}

export function readProcessStartToken(
  pid,
  { platform = process.platform, spawnCommand = spawnSync } = {},
) {
  if (!Number.isSafeInteger(pid) || pid <= 1) {
    throw new Error(`invalid process PID: ${pid}`);
  }
  if (platform === "win32") {
    const command = [
      "$process = Get-Process -Id $args[0] -ErrorAction SilentlyContinue",
      "if ($null -eq $process) { exit 3 }",
      "$process.StartTime.ToUniversalTime().Ticks",
    ].join("; ");
    const output = commandOutput(
      spawnCommand(
        "powershell.exe",
        ["-NoProfile", "-NonInteractive", "-Command", command, String(pid)],
        { encoding: "utf8", windowsHide: true },
      ),
      "powershell Get-Process",
      3,
    );
    return output === null ? null : `win32:${output}`;
  }

  const output = commandOutput(
    spawnCommand("ps", ["-o", "lstart=", "-p", String(pid)], {
      encoding: "utf8",
    }),
    "ps",
    1,
  );
  return output === null ? null : `${platform}:${output}`;
}

export function inspectDevProcessIdentity(
  state,
  { platform = process.platform, readStartToken = readProcessStartToken } = {},
) {
  if (state.platform !== platform) {
    return {
      childRunning: null,
      matches: false,
      reason: `recorded platform ${state.platform} does not match ${platform}`,
    };
  }

  const processStartToken = readStartToken(state.pid, { platform });
  if (processStartToken === null) {
    return {
      childRunning: false,
      matches: false,
      reason: `recorded process PID ${state.pid} is no longer running`,
    };
  }
  if (processStartToken !== state.processStartToken) {
    return {
      childRunning: true,
      matches: false,
      reason: `PID ${state.pid} belongs to a different process start identity`,
    };
  }

  const parentStartToken = readStartToken(state.parentPid, { platform });
  if (parentStartToken === null) {
    return {
      childRunning: true,
      matches: false,
      reason: `recorded launcher PID ${state.parentPid} is no longer running`,
    };
  }
  if (parentStartToken !== state.parentStartToken) {
    return {
      childRunning: true,
      matches: false,
      reason: `launcher PID ${state.parentPid} belongs to a different process start identity`,
    };
  }
  return { childRunning: true, matches: true, reason: null };
}

export function devProcessStatePath(repoRoot) {
  return join(repoRoot, ".patchbay-dev", "dev-process.json");
}

export function devLifecycleLockPath(
  repoRoot,
  { spawnCommand = spawnSync } = {},
) {
  const gitDir = commandOutput(
    spawnCommand(
      "git",
      ["-C", resolve(repoRoot), "rev-parse", "--absolute-git-dir"],
      { encoding: "utf8" },
    ),
    "git rev-parse --absolute-git-dir",
  );
  return join(gitDir, "patchbay-dev-lifecycle.lock");
}

export async function acquireDevLifecycleLock(
  repoRoot,
  { resolveLockPath = devLifecycleLockPath } = {},
) {
  const lockPath = resolveLockPath(repoRoot);
  try {
    await mkdir(lockPath);
  } catch (error) {
    if (error?.code === "EEXIST") {
      throw new Error(
        `complete development lifecycle is busy for this checkout; inspect ${lockPath}`,
      );
    }
    throw error;
  }
  return async () => {
    await rm(lockPath, { recursive: true, force: true });
  };
}

export function parseDevProcessState(raw, repoRoot) {
  let state;
  try {
    state = JSON.parse(raw);
  } catch {
    throw new Error("invalid complete development process state");
  }
  if (
    !Number.isSafeInteger(state.pid) ||
    state.pid <= 1 ||
    !Number.isSafeInteger(state.parentPid) ||
    state.parentPid <= 1 ||
    typeof state.platform !== "string" ||
    typeof state.processStartToken !== "string" ||
    state.processStartToken.length === 0 ||
    typeof state.parentStartToken !== "string" ||
    state.parentStartToken.length === 0 ||
    resolve(state.repoRoot || "") !== resolve(repoRoot)
  ) {
    throw new Error("invalid complete development process state");
  }
  return state;
}

export async function readDevProcessState(repoRoot) {
  try {
    return parseDevProcessState(
      await readFile(devProcessStatePath(repoRoot), "utf8"),
      repoRoot,
    );
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}

export async function writeDevProcessState(repoRoot, state) {
  const statePath = devProcessStatePath(repoRoot);
  await mkdir(dirname(statePath), { recursive: true });
  const handle = await open(statePath, "wx", 0o600);
  try {
    await handle.writeFile(
      `${JSON.stringify({ ...state, repoRoot: resolve(repoRoot) }, null, 2)}\n`,
      "utf8",
    );
  } catch (error) {
    await handle.close();
    await rm(statePath, { force: true });
    throw error;
  }
  await handle.close();
}

export async function clearDevProcessState(repoRoot, expectedPid) {
  if (expectedPid) {
    try {
      const current = await readDevProcessState(repoRoot);
      if (current && current.pid !== expectedPid) return;
    } catch {
      // A malformed state file is not owned by this launcher.
      return;
    }
  }
  await rm(devProcessStatePath(repoRoot), { force: true });
}

function signalExists(pid, killProcess) {
  try {
    killProcess(pid, 0);
    return true;
  } catch (error) {
    if (error?.code === "ESRCH") return false;
    if (error?.code === "EPERM") return true;
    throw error;
  }
}

export function devProcessTreeIsRunning(
  state,
  { platform = process.platform, killProcess = process.kill } = {},
) {
  return signalExists(
    platform === "win32" ? state.pid : -state.pid,
    killProcess,
  );
}

export function devProcessLauncherIsRunning(
  state,
  { killProcess = process.kill } = {},
) {
  return signalExists(state.parentPid, killProcess);
}

export function planDevProcessSignal(platform, pid, { force = false } = {}) {
  if (platform === "win32") {
    return {
      command: "taskkill.exe",
      args: ["/PID", String(pid), "/T", "/F"],
    };
  }
  return {
    pid: -pid,
    signal: force ? "SIGKILL" : "SIGTERM",
  };
}

export function signalDevProcessTree(
  state,
  {
    platform = process.platform,
    force = false,
    killProcess = process.kill,
    spawnCommand = spawnSync,
  } = {},
) {
  const plan = planDevProcessSignal(platform, state.pid, { force });
  if (plan.command) {
    const result = spawnCommand(plan.command, plan.args, { stdio: "ignore" });
    if (result.error) throw result.error;
    if (
      result.status !== 0 &&
      devProcessTreeIsRunning(state, { platform, killProcess })
    ) {
      throw new Error(`taskkill failed with exit code ${result.status}`);
    }
    return;
  }
  try {
    killProcess(plan.pid, plan.signal);
  } catch (error) {
    if (error?.code !== "ESRCH") throw error;
  }
}
