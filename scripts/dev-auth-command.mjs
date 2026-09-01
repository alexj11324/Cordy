#!/usr/bin/env node

import { spawn } from "node:child_process";
import { pathToFileURL } from "node:url";

import {
  bootstrapDevClerkAuth,
  scopedDevClerkEnvironment,
  withoutDevClerkEnvironment,
} from "./dev-clerk-auth.mjs";

export async function runAuthenticatedDevCommand({
  scope,
  command,
  args = [],
  env = process.env,
  cwd = process.cwd(),
  bootstrap = bootstrapDevClerkAuth,
  spawnImpl = spawn,
} = {}) {
  if (!command) throw new Error("Authenticated development command is missing.");
  const auth = await bootstrap({ env });
  const baseEnv = withoutDevClerkEnvironment(env);
  const child = spawnImpl(command, args, {
    cwd,
    stdio: "inherit",
    env: {
      ...baseEnv,
      ...scopedDevClerkEnvironment(auth.authEnv, scope),
    },
  });
  const forwardSignal = (signal) => child.kill(signal);
  process.once("SIGINT", forwardSignal);
  process.once("SIGTERM", forwardSignal);
  return new Promise((resolveRun, rejectRun) => {
    child.once("error", rejectRun);
    child.once("close", (code, signal) => {
      process.removeListener("SIGINT", forwardSignal);
      process.removeListener("SIGTERM", forwardSignal);
      if (code !== null) resolveRun(code);
      else resolveRun(signal === "SIGINT" ? 130 : 143);
    });
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [scope, command, ...args] = process.argv.slice(2);
  runAuthenticatedDevCommand({ scope, command, args })
    .then((status) => {
      process.exitCode = status;
    })
    .catch((error) => {
      console.error(`✗ ${error.message}`);
      process.exitCode = 1;
    });
}
