#!/usr/bin/env node

import { pathToFileURL } from "node:url";

import {
  defaultDevCliCacheDir,
  pruneDevRuntimeCache,
} from "./dev-cli-cache.mjs";

export function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024;
    unit = units[index];
  }
  return `${value.toFixed(1)} ${unit}`;
}

async function main() {
  const cacheRoot = defaultDevCliCacheDir();
  const result = await pruneDevRuntimeCache({ cacheRoot });
  console.log(
    `[dev-runtime] cache ${formatBytes(result.totalBytes)} -> ${formatBytes(result.remainingBytes)}; removed ${result.removedCount} entries; protected ${result.protectedFingerprintCount} complete fingerprints`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(`✗ Development runtime cache cleanup failed: ${error.message}`);
    process.exitCode = 1;
  });
}
