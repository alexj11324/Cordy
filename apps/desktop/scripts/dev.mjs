#!/usr/bin/env node
// Internal Electron phase of the complete `pnpm dev` launcher.
//
// Derives per-worktree isolation env (renderer port + app name) so multiple
// worktrees can run `pnpm dev:desktop` side-by-side, then brands the dev
// Electron and starts electron-vite with the augmented env. The parent
// scripts/dev.sh process has already prepared the isolated DB and healthy
// backend. The parent has also staged the source-matched CLI/backend/migration
// runtime set. This phase runs the capability doctor before opening a window;
// there is no UI-only fallback.

import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { envWithLocalBins } from "./package.mjs";
import { planDevCommands } from "./dev-plan.mjs";
import {
  clearDevClerkEnvironment,
  withoutDevClerkEnvironment,
} from "../../../scripts/dev-clerk-auth.mjs";
import {
  applyDevRuntimeAppIdentity,
  parseDevRuntimeArgs,
} from "../../../scripts/dev-runtime-profile.mjs";
import {
  applyMacOSDevElectronEnv,
  applyWorktreeDevEnv,
  repoRootFromScriptDir,
} from "./worktree-dev-env.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const acceptanceReleaseScript = resolve(
  here,
  "../../../scripts/release-dev-acceptance-port.mjs",
);
const { electronArgs } = parseDevRuntimeArgs(process.argv.slice(2));

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
applyDevRuntimeAppIdentity(process.env);
const sanitizedChildEnv = withoutDevClerkEnvironment(process.env);

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
for (const step of planDevCommands(electronArgs, {
  nodePath: process.execPath,
  scriptsDir: here,
})) {
  const isDoctor =
    step.args[0]?.endsWith("dev-environment-doctor.mjs") === true;
  const isElectronVite = step.command === "electron-vite";
  if (
    isElectronVite &&
    (process.env.PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT ||
      process.env.PATCHBAY_DEV_ACCEPTANCE_RELEASE_TOKEN)
  ) {
    // Credentialed acceptance keeps the loopback CDP port reserved while the
    // doctor and branding phases run. Release it immediately before
    // electron-vite can spawn Electron, then remove the handoff token from
    // every child environment so it cannot be inherited by the app.
    run(process.execPath, [acceptanceReleaseScript], { env: process.env });
    delete process.env.PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT;
    delete process.env.PATCHBAY_DEV_ACCEPTANCE_RELEASE_TOKEN;
    delete sanitizedChildEnv.PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT;
    delete sanitizedChildEnv.PATCHBAY_DEV_ACCEPTANCE_RELEASE_TOKEN;
  }
  const electronEnv =
    isElectronVite && process.env.PATCHBAY_DEV_ELECTRON_DIST_PATH
      ? {
          ...sanitizedChildEnv,
          ELECTRON_OVERRIDE_DIST_PATH:
            process.env.PATCHBAY_DEV_ELECTRON_DIST_PATH,
        }
      : sanitizedChildEnv;
  run(step.command, step.args, {
    shell: isElectronVite && isWin,
    // Only the doctor may see explicit process-only Clerk credentials. The
    // brander and Electron/Vite receive a copied environment with all auth
    // secrets removed; Electron obtains its normal session through the UI and
    // must never inherit server-side Clerk material.
    env: isDoctor
      ? process.env
      : isElectronVite
        ? envWithLocalBins(electronEnv)
        : sanitizedChildEnv,
  });
  if (isDoctor) clearDevClerkEnvironment();
}
