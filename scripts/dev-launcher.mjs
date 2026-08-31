#!/usr/bin/env node

import { spawn } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { ensureDevCheckoutEnv } from "../apps/desktop/scripts/dev-checkout-env.mjs";
import {
  clearDevProcessState,
  devProcessLauncherIsRunning,
  devProcessTreeIsRunning,
  readDevProcessState,
  signalDevProcessTree,
  writeDevProcessState,
} from "./dev-process.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..");

export function planCompleteDevLauncher(
  platform,
  argv,
  { repoRoot = defaultRepoRoot } = {},
) {
  if (platform === "win32") {
    return {
      command: "powershell.exe",
      args: [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        join(repoRoot, "scripts", "dev.ps1"),
        ...argv,
      ],
    };
  }
  return {
    command: "bash",
    args: [join(repoRoot, "scripts", "dev.sh"), ...argv],
  };
}

export async function runCompleteDev({
  repoRoot = defaultRepoRoot,
  env = process.env,
  platform = process.platform,
  argv = process.argv.slice(2),
} = {}) {
  await ensureDevCheckoutEnv({ repoRoot, env });
  const existing = await readDevProcessState(repoRoot);
  if (existing) {
    const treeRunning = devProcessTreeIsRunning(existing, { platform });
    if (treeRunning && devProcessLauncherIsRunning(existing)) {
      throw new Error(
        `complete development is already running for this checkout (PID ${existing.pid}); run make stop first`,
      );
    }
    if (treeRunning) {
      throw new Error(
        `tracked development process group ${existing.pid} is still running without its launcher; inspect it before removing ${repoRoot}/.patchbay-dev/dev-process.json`,
      );
    }
    await clearDevProcessState(repoRoot, existing.pid);
  }

  const step = planCompleteDevLauncher(platform, argv, { repoRoot });
  const child = spawn(step.command, step.args, {
    cwd: repoRoot,
    env,
    stdio: "inherit",
    detached: platform !== "win32",
  });
  await new Promise((resolveSpawn, rejectSpawn) => {
    child.once("spawn", resolveSpawn);
    child.once("error", rejectSpawn);
  });
  const completion = new Promise((resolveCompletion) => {
    child.once("close", (code, signal) => resolveCompletion({ code, signal }));
  });

  const state = {
    pid: child.pid,
    parentPid: process.pid,
    platform,
    startedAt: new Date().toISOString(),
  };
  try {
    await writeDevProcessState(repoRoot, state);
  } catch (error) {
    signalDevProcessTree(state, { platform, force: true });
    if (error?.code === "EEXIST") {
      throw new Error(
        "another complete development launch claimed this checkout; run make stop before retrying",
      );
    }
    throw error;
  }

  const forwardSignal = () => {
    try {
      signalDevProcessTree(state, { platform });
    } catch {
      // The child may have exited between the signal and this handler.
    }
  };
  process.once("SIGINT", forwardSignal);
  process.once("SIGTERM", forwardSignal);
  try {
    const { code, signal } = await completion;
    if (code !== null) return code;
    return signal === "SIGINT" ? 130 : 143;
  } finally {
    process.removeListener("SIGINT", forwardSignal);
    process.removeListener("SIGTERM", forwardSignal);
    await clearDevProcessState(repoRoot, child.pid);
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  runCompleteDev()
    .then((status) => {
      process.exitCode = status;
    })
    .catch((error) => {
      console.error(`✗ ${error.message}`);
      process.exitCode = 1;
    });
}
