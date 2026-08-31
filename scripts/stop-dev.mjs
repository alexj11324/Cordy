#!/usr/bin/env node

import { setTimeout as delay } from "node:timers/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  clearDevProcessState,
  devProcessLauncherIsRunning,
  devProcessTreeIsRunning,
  inspectDevProcessIdentity,
  readDevProcessState,
  signalDevProcessTree,
} from "./dev-process.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..");

export async function stopCompleteDev({
  repoRoot = defaultRepoRoot,
  platform = process.platform,
  log = console,
  readStartToken,
  killProcess = process.kill,
  spawnCommand,
  gracePeriodMs = 5_000,
} = {}) {
  const state = await readDevProcessState(repoRoot);
  if (!state) {
    log.log("No complete development process is tracked for this checkout.");
    return { stopped: false, stale: false };
  }
  const identity = inspectDevProcessIdentity(state, {
    platform,
    ...(readStartToken ? { readStartToken } : {}),
  });
  if (identity.childRunning === false) {
    if (
      platform === "win32" ||
      devProcessTreeIsRunning(state, { platform, killProcess })
    ) {
      throw new Error(
        `refusing to clear process state for PID ${state.pid}: its recorded leader exited but the full process tree cannot be confirmed stopped`,
      );
    }
    await clearDevProcessState(repoRoot, state.pid);
    log.log(`Removed stale development process state for PID ${state.pid}.`);
    return { stopped: false, stale: true };
  }
  if (!identity.matches) {
    throw new Error(
      `refusing to signal process group ${state.pid}: ${identity.reason}; inspect the process before removing ${repoRoot}/.patchbay-dev/dev-process.json`,
    );
  }
  if (!devProcessTreeIsRunning(state, { platform, killProcess })) {
    throw new Error(
      `refusing to clear process state for PID ${state.pid}: its identity matches but its tracked process tree cannot be confirmed`,
    );
  }
  if (!devProcessLauncherIsRunning(state, { killProcess })) {
    throw new Error(
      `refusing to signal process group ${state.pid}: its launcher identity matches but launcher PID ${state.parentPid} cannot be confirmed`,
    );
  }

  const signalIdentity = inspectDevProcessIdentity(state, {
    platform,
    ...(readStartToken ? { readStartToken } : {}),
  });
  if (!signalIdentity.matches) {
    throw new Error(
      `refusing to signal process group ${state.pid}: ${signalIdentity.reason}; its identity changed immediately before stop`,
    );
  }

  log.log(`Stopping complete development process tree (PID ${state.pid})...`);
  signalDevProcessTree(state, { platform, killProcess, spawnCommand });
  if (platform !== "win32") {
    const deadline = Date.now() + gracePeriodMs;
    while (
      Date.now() < deadline &&
      devProcessTreeIsRunning(state, { platform, killProcess })
    ) {
      await delay(100);
    }
    if (devProcessTreeIsRunning(state, { platform, killProcess })) {
      const forceIdentity = inspectDevProcessIdentity(state, {
        platform,
        ...(readStartToken ? { readStartToken } : {}),
      });
      if (!forceIdentity.matches) {
        throw new Error(
          `refusing to force process group ${state.pid}: ${forceIdentity.reason}; inspect the process before removing ${repoRoot}/.patchbay-dev/dev-process.json`,
        );
      }
      signalDevProcessTree(state, {
        platform,
        force: true,
        killProcess,
        spawnCommand,
      });
      const forceDeadline = Date.now() + gracePeriodMs;
      while (
        Date.now() < forceDeadline &&
        devProcessTreeIsRunning(state, { platform, killProcess })
      ) {
        await delay(100);
      }
      if (devProcessTreeIsRunning(state, { platform, killProcess })) {
        throw new Error(
          `process group ${state.pid} remained alive after SIGKILL; refusing to clear its development process state`,
        );
      }
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
  const repoRootIndex = process.argv.indexOf("--repo-root");
  if (repoRootIndex !== -1 && !process.argv[repoRootIndex + 1]) {
    console.error("✗ --repo-root requires a path");
    process.exitCode = 1;
  } else {
    const repoRoot =
      repoRootIndex === -1
        ? defaultRepoRoot
        : resolve(process.argv[repoRootIndex + 1]);
    stopCompleteDev({ repoRoot }).catch((error) => {
      console.error(`✗ ${error.message}`);
      process.exitCode = 1;
    });
  }
}
