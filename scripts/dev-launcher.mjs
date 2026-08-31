#!/usr/bin/env node

import { spawn } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { ensureDevCheckoutEnv } from "../apps/desktop/scripts/dev-checkout-env.mjs";
import { bootstrapDevClerkAuth } from "./dev-clerk-auth.mjs";
import {
  applyDevRuntimeProfile,
  assertDevRuntimeOverridesCompatible,
  parseDevRuntimeArgs,
  resolveDevRuntimeProfile,
} from "./dev-runtime-profile.mjs";
import {
  acquireDevLifecycleLock,
  clearDevProcessState,
  devProcessLauncherIsRunning,
  devProcessTreeIsRunning,
  inspectDevProcessIdentity,
  readProcessStartToken,
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
  const releaseLifecycleLock = await acquireDevLifecycleLock(repoRoot);
  const inheritedEnv = { ...env };
  let child;
  let completion;
  let state;
  try {
    await ensureDevCheckoutEnv({ repoRoot, env });
    const { mode } = parseDevRuntimeArgs(argv);
    assertDevRuntimeOverridesCompatible(mode, inheritedEnv);
    applyDevRuntimeProfile(
      env,
      resolveDevRuntimeProfile(mode, env),
    );
    if (mode !== "hosted") await bootstrapDevClerkAuth({ env });
    const existing = await readDevProcessState(repoRoot);
    if (existing) {
      const identity = inspectDevProcessIdentity(existing, { platform });
      if (identity.childRunning === false) {
        if (
          platform === "win32" ||
          devProcessTreeIsRunning(existing, { platform })
        ) {
          throw new Error(
            `refusing to replace complete development process state: recorded leader PID ${existing.pid} exited but the full process tree cannot be confirmed stopped; inspect ${repoRoot}/.patchbay-dev/dev-process.json`,
          );
        }
        await clearDevProcessState(repoRoot, existing.pid);
      } else if (!identity.matches) {
        throw new Error(
          `refusing to replace complete development process state: ${identity.reason}; inspect ${repoRoot}/.patchbay-dev/dev-process.json`,
        );
      } else if (
        devProcessTreeIsRunning(existing, { platform }) &&
        devProcessLauncherIsRunning(existing)
      ) {
        throw new Error(
          `complete development is already running for this checkout (PID ${existing.pid}); run make stop first`,
        );
      } else {
        throw new Error(
          `tracked development process group ${existing.pid} has an inconsistent process tree; inspect it before removing ${repoRoot}/.patchbay-dev/dev-process.json`,
        );
      }
    }

    const step = planCompleteDevLauncher(platform, argv, { repoRoot });
    child = spawn(step.command, step.args, {
      cwd: repoRoot,
      env,
      stdio: "inherit",
      detached: platform !== "win32",
    });
    await new Promise((resolveSpawn, rejectSpawn) => {
      child.once("spawn", resolveSpawn);
      child.once("error", rejectSpawn);
    });
    completion = new Promise((resolveCompletion) => {
      child.once("close", (code, signal) =>
        resolveCompletion({ code, signal }),
      );
    });

    state = {
      pid: child.pid,
      parentPid: process.pid,
      platform,
      startedAt: new Date().toISOString(),
    };
    try {
      state.processStartToken = readProcessStartToken(child.pid, { platform });
      state.parentStartToken = readProcessStartToken(process.pid, { platform });
      if (!state.processStartToken || !state.parentStartToken) {
        throw new Error(
          "could not capture complete development process identity",
        );
      }
    } catch (error) {
      signalDevProcessTree(state, { platform, force: true });
      throw error;
    }
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
  } finally {
    await releaseLifecycleLock();
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
