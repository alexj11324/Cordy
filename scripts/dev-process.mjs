import { spawnSync } from "node:child_process";
import { mkdir, open, readFile, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

export function devProcessStatePath(repoRoot) {
  return join(repoRoot, ".patchbay-dev", "dev-process.json");
}

export function parseDevProcessState(raw, repoRoot) {
  const state = JSON.parse(raw);
  if (
    !Number.isSafeInteger(state.pid) ||
    state.pid <= 1 ||
    !Number.isSafeInteger(state.parentPid) ||
    state.parentPid <= 1 ||
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
