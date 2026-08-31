#!/usr/bin/env node

import { spawn } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { ensureDevCheckoutEnv } from "../apps/desktop/scripts/dev-checkout-env.mjs";
import {
  bootstrapDevClerkAuth,
  scopedDevClerkEnvironment,
  withoutDevClerkEnvironment,
} from "./dev-clerk-auth.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..");

export async function runStandaloneWeb({
  repoRoot = defaultRepoRoot,
  env = process.env,
  argv = process.argv.slice(2),
} = {}) {
  await ensureDevCheckoutEnv({ repoRoot, env });
  const auth = await bootstrapDevClerkAuth({ env });
  const baseEnv = withoutDevClerkEnvironment(env);
  const port = env.FRONTEND_PORT || "3000";
  const child = spawn(
    process.execPath,
    ["node_modules/next/dist/bin/next", "dev", "--webpack", "--port", port, ...argv],
    {
      cwd: join(repoRoot, "apps", "web"),
      env: {
        ...baseEnv,
        ...scopedDevClerkEnvironment(auth.authEnv, "web"),
      },
      stdio: "inherit",
    },
  );
  return new Promise((resolveRun, rejectRun) => {
    child.once("error", rejectRun);
    child.once("close", (code, signal) => {
      if (code !== null) resolveRun(code);
      else resolveRun(signal === "SIGINT" ? 130 : 143);
    });
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runStandaloneWeb()
    .then((status) => {
      process.exitCode = status;
    })
    .catch((error) => {
      console.error(`✗ ${error.message}`);
      process.exitCode = 1;
    });
}
