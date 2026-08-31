#!/usr/bin/env node

import { setTimeout as delay } from "node:timers/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  clearDevProcessState,
  devProcessLauncherIsRunning,
  devProcessTreeIsRunning,
  readDevProcessState,
  signalDevProcessTree,
} from "./dev-process.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..");

export async function stopCompleteDev({
  repoRoot = defaultRepoRoot,
  platform = process.platform,
  log = console,
} = {}) {
  const state = await readDevProcessState(repoRoot);
  if (!state) {
    log.log("No complete development process is tracked for this checkout.");
    return { stopped: false, stale: false };
  }
  const treeRunning = devProcessTreeIsRunning(state, { platform });
  if (!treeRunning) {
    await clearDevProcessState(repoRoot, state.pid);
    log.log(`Removed stale development process state for PID ${state.pid}.`);
    return { stopped: false, stale: true };
  }
  if (!devProcessLauncherIsRunning(state)) {
    throw new Error(
      `refusing to signal orphaned process group ${state.pid}: its recorded launcher PID ${state.parentPid} is no longer running; inspect the process before removing ${repoRoot}/.patchbay-dev/dev-process.json`,
    );
  }

  log.log(`Stopping complete development process tree (PID ${state.pid})...`);
  signalDevProcessTree(state, { platform });
  if (platform !== "win32") {
    const deadline = Date.now() + 5_000;
    while (
      Date.now() < deadline &&
      devProcessTreeIsRunning(state, { platform })
    ) {
      await delay(100);
    }
    if (devProcessTreeIsRunning(state, { platform })) {
      signalDevProcessTree(state, { platform, force: true });
    }
  }
  await clearDevProcessState(repoRoot, state.pid);
  log.log("Complete Electron development stack stopped.");
  return { stopped: true, stale: false };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  stopCompleteDev().catch((error) => {
    console.error(`✗ ${error.message}`);
    process.exitCode = 1;
  });
}
