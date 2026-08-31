#!/usr/bin/env node
// Internal Electron phase of the complete `pnpm dev` launcher.
//
// Derives per-worktree isolation env (renderer port, app name, callback scheme) so multiple
// worktrees can run `pnpm dev:desktop` side-by-side, then brands the dev
// Electron and starts electron-vite with the augmented env. The parent
// scripts/dev.sh process has already prepared the isolated DB and healthy
// backend. The parent has also staged the source-matched CLI/backend/migration
// runtime set. This phase runs the capability doctor before opening a window;
// there is no UI-only fallback.

import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

import { envWithLocalBins } from "./package.mjs";
import { planDevCommands } from "./dev-plan.mjs";
import {
  applyMacOSDevElectronEnv,
  applyWorktreeDevEnv,
  repoRootFromScriptDir,
} from "./worktree-dev-env.mjs";

const here = dirname(fileURLToPath(import.meta.url));

applyWorktreeDevEnv(process.env, {
  root: repoRootFromScriptDir(here),
  log: true,
});
const require = createRequire(import.meta.url);
const electronVersion = require("electron/package.json").version;
applyMacOSDevElectronEnv(process.env, {
  home: homedir(),
  electronVersion,
});

function run(command, args, { shell = false, env = process.env } = {}) {
  const result = spawnSync(command, args, {
    stdio: "inherit",
    env,
    shell,
  });
  if (result.error) {
    console.error(
      `[dev:desktop] failed to run ${command}: ${result.error.message}`,
    );
    process.exit(1);
  }
  if (result.status !== 0) process.exit(result.status ?? 1);
}

const isWin = process.platform === "win32";
// electron-vite's bin lands in apps/desktop/node_modules/.bin under the
// isolated linker but only in the repo-root .bin under the hoisted linker
// (.npmrc node-linker=hoisted); envWithLocalBins puts both on PATH.
for (const step of planDevCommands(process.argv.slice(2), {
  nodePath: process.execPath,
  scriptsDir: here,
})) {
  const isElectronVite = step.command === "electron-vite";
  const stepEnv =
    isElectronVite && process.env.PATCHBAY_DEV_ELECTRON_DIST_PATH
      ? {
          ...process.env,
          ELECTRON_OVERRIDE_DIST_PATH:
            process.env.PATCHBAY_DEV_ELECTRON_DIST_PATH,
        }
      : process.env;
  run(step.command, step.args, {
    shell: isElectronVite && isWin,
    env: isElectronVite ? envWithLocalBins(stepEnv) : stepEnv,
  });
}
