#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { chmod, copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");

function option(argv, name) {
  const index = argv.indexOf(name);
  return index === -1 ? "" : argv[index + 1] || "";
}

async function sha256File(path) {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}

export async function writeCliArtifact({
  sourceBinary,
  outputDirectory,
  rustTarget,
  runtimePlatform,
  runtimeArch,
  version,
  commit,
}) {
  const binaryName = runtimePlatform === "win32" ? "patchbay.exe" : "patchbay";
  await mkdir(outputDirectory, { recursive: true });
  const outputBinary = join(outputDirectory, binaryName);
  await copyFile(sourceBinary, outputBinary);
  if (runtimePlatform !== "win32") await chmod(outputBinary, 0o755);
  const manifest = {
    schemaVersion: 1,
    commit,
    version,
    rustTarget,
    runtimePlatform,
    runtimeArch,
    binaryName,
    sha256: await sha256File(outputBinary),
  };
  await writeFile(
    join(outputDirectory, "manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  return manifest;
}

async function main() {
  const argv = process.argv.slice(2);
  const sourceBinary = option(argv, "--binary");
  const outputDirectory = option(argv, "--output");
  const rustTarget = option(argv, "--target");
  const runtimePlatform = option(argv, "--platform");
  const runtimeArch = option(argv, "--arch");
  const version = option(argv, "--version");
  if (
    !sourceBinary ||
    !outputDirectory ||
    !rustTarget ||
    !runtimePlatform ||
    !runtimeArch ||
    !version
  ) {
    throw new Error(
      "usage: write-cli-artifact-manifest.mjs --binary PATH --output DIR --target TRIPLE --platform PLATFORM --arch ARCH --version VERSION",
    );
  }
  const commit = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim();
  const manifest = await writeCliArtifact({
    sourceBinary: resolve(sourceBinary),
    outputDirectory: resolve(outputDirectory),
    rustTarget,
    runtimePlatform,
    runtimeArch,
    version,
    commit,
  });
  console.log(
    `[release-cli] staged ${basename(sourceBinary)} for ${manifest.commit} ${runtimePlatform}/${runtimeArch}`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
