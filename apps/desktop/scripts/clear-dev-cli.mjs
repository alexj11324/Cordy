#!/usr/bin/env node

import { rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const defaultArtifactDir = join(scriptsDir, "..", "resources", "bin");

export async function clearDevCliArtifact(
  artifactDir = defaultArtifactDir,
) {
  await rm(artifactDir, { recursive: true, force: true });
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  clearDevCliArtifact().catch((error) => {
    console.error(`[dev:desktop] failed to clear stale CLI: ${error.message}`);
    process.exitCode = 1;
  });
}
