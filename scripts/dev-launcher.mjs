#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { ensureDevCheckoutEnv } from "../apps/desktop/scripts/dev-checkout-env.mjs";

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
  const step = planCompleteDevLauncher(platform, argv, { repoRoot });
  const result = spawnSync(step.command, step.args, {
    cwd: repoRoot,
    env,
    stdio: "inherit",
  });
  if (result.error) {
    throw new Error(
      `failed to launch complete development environment with ${step.command}: ${result.error.message}`,
    );
  }
  return result.status ?? 1;
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
