#!/usr/bin/env node
// Dev launcher for `pnpm dev:desktop`.
//
// Derives per-worktree isolation env (renderer port + app name) so multiple
// worktrees can run `pnpm dev:desktop` side-by-side, then brands the dev
// Electron and starts electron-vite with the augmented env. Rust is opt-in via
// the public `pnpm dev:desktop:rust` command: ordinary renderer/main-process
// edits stay on Vite's fast feedback loop, while contributors changing
// server-rs can explicitly bundle a source-matched incremental development CLI.
// Returning to the default command clears that source artifact before launch.

import { spawnSync } from "node:child_process";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

import { envWithLocalBins } from "./package.mjs";
import { planDevCommands } from "./dev-plan.mjs";
import {
  applyWorktreeDevEnv,
  repoRootFromScriptDir,
} from "./worktree-dev-env.mjs";

const here = dirname(fileURLToPath(import.meta.url));

applyWorktreeDevEnv(process.env, {
  root: repoRootFromScriptDir(here),
  log: true,
});

function run(command, args, { shell = false, env = process.env } = {}) {
  const result = spawnSync(command, args, {
    stdio: "inherit",
    env,
    shell,
  });
  if (result.error) {
    console.error(`[dev:desktop] failed to run ${command}: ${result.error.message}`);
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
  run(step.command, step.args, {
    shell: isElectronVite && isWin,
    env: isElectronVite ? envWithLocalBins(process.env) : process.env,
  });
}
