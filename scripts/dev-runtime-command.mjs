#!/usr/bin/env node

import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  devRuntimeComponents,
  prepareDevRuntime,
} from "../apps/desktop/scripts/prepare-dev-runtime.mjs";
import { loadDevCheckoutEnv } from "../apps/desktop/scripts/dev-checkout-env.mjs";
import {
  bootstrapDevClerkAuth,
  scopedDevClerkEnvironment,
  withoutDevClerkEnvironment,
} from "./dev-clerk-auth.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..");

export async function runDevRuntimeCommand({
  componentId,
  args = [],
  repoRoot = defaultRepoRoot,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
} = {}) {
  loadDevCheckoutEnv({ repoRoot, env });
  await prepareDevRuntime({ repoRoot, env, platform, arch });
  const auth =
    componentId === "backend" ? await bootstrapDevClerkAuth({ env }) : null;
  const component = devRuntimeComponents({ repoRoot, platform, arch }).find(
    ({ id }) => id === componentId,
  );
  if (!component) throw new Error(`Unknown development runtime: ${componentId}`);
  const child = spawn(component.destinationBinary, args, {
    cwd: resolve(repoRoot, "server-rs"),
    env: auth
      ? {
          ...withoutDevClerkEnvironment(env),
          ...scopedDevClerkEnvironment(auth.authEnv, "backend"),
        }
      : withoutDevClerkEnvironment(env),
    stdio: "inherit",
  });
  return new Promise((resolveRun, rejectRun) => {
    child.once("error", rejectRun);
    child.once("close", (code, signal) => {
      if (code !== null) resolveRun(code);
      else resolveRun(signal === "SIGINT" ? 130 : 143);
    });
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [componentId, ...args] = process.argv.slice(2);
  runDevRuntimeCommand({ componentId, args })
    .then((status) => {
      process.exitCode = status;
    })
    .catch((error) => {
      console.error(`✗ ${error.message}`);
      process.exitCode = 1;
    });
}
