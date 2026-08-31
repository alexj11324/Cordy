#!/usr/bin/env node

import { resolve } from "node:path";

import { createWorktreeEnvFile } from "../apps/desktop/scripts/dev-checkout-env.mjs";

const repoRoot = process.cwd();
const envFile = resolve(repoRoot, process.argv[2] || ".env.worktree");

createWorktreeEnvFile({
  repoRoot,
  envFile,
  force: process.env.FORCE === "1",
  worktreeName: process.env.WORKTREE_NAME,
})
  .then(({ offset }) => {
    console.log(`Generated ${envFile} with isolated port offset ${offset}.`);
    console.log("Next step: pnpm dev");
  })
  .catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
