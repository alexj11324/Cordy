#!/usr/bin/env node
// Builds the `patchbay` CLI from server/cmd/patchbay and copies the binary
// into apps/desktop/resources/bin/ so electron-builder can package the exact
// source revision. Development uses prepare-dev-runtime.mjs instead; ordinary
// frontend/Electron builds do not prepare a CLI.
//
// ldflags mirror `make build` so `patchbay --version` reports a meaningful
// version / commit / date.
//
// A missing Go toolchain or a genuine compile error is fatal. Packaging an app
// that silently falls back to a release CLI would make the Desktop renderer
// and backend advertise one revision while executing another.

import { access, chmod, copyFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { createHash } from "node:crypto";
import { execFileSync, execSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const serverDir = join(repoRoot, "server");

const PLATFORM_TO_GOOS = {
  darwin: "darwin",
  linux: "linux",
  win32: "windows",
};

const SUPPORTED_ARCHS = new Set(["x64", "arm64"]);

function runtimePlatformFromArgs(argv) {
  const flagIndex = argv.indexOf("--target-platform");
  if (flagIndex === -1) return process.platform;
  return argv[flagIndex + 1] ?? "";
}

function runtimeArchFromArgs(argv) {
  const flagIndex = argv.indexOf("--target-arch");
  if (flagIndex === -1) return process.arch;
  return argv[flagIndex + 1] ?? "";
}

function normalizeRuntimePlatform(platform) {
  if (platform in PLATFORM_TO_GOOS) return platform;
  throw new Error(
    `[bundle-cli] unsupported target platform: ${platform}. ` +
      "Use darwin, linux, or win32.",
  );
}

function normalizeRuntimeArch(arch) {
  if (SUPPORTED_ARCHS.has(arch)) return arch;
  throw new Error(
    `[bundle-cli] unsupported target architecture: ${arch}. ` +
      "Use x64 or arm64.",
  );
}

function binaryNameForPlatform(platform) {
  return platform === "win32" ? "patchbay.exe" : "patchbay";
}

const targetPlatform = normalizeRuntimePlatform(
  runtimePlatformFromArgs(process.argv.slice(2)),
);
const targetArch = normalizeRuntimeArch(runtimeArchFromArgs(process.argv.slice(2)));
const goos = PLATFORM_TO_GOOS[targetPlatform];
const goarch = targetArch === "x64" ? "amd64" : targetArch;
const binName = binaryNameForPlatform(targetPlatform);
const srcBinary = join(serverDir, "bin", `${goos}-${goarch}`, binName);
const destDir = join(repoRoot, "apps", "desktop", "resources", "bin");
const destBinary = join(destDir, binName);

// Hand git arguments straight to the binary (no shell). A match pattern like
// `v[0-9]*` must reach git as one literal argument; routing it through a shell
// string breaks on Windows, where cmd.exe keeps the POSIX single quotes and
// git matches no tag — degrading the bundled CLI's version to the
// 0.0.0-g<hash> fallback.
function git(...args) {
  try {
    return execFileSync("git", args, { encoding: "utf-8" }).trim();
  } catch {
    return "";
  }
}

async function exists(p) {
  try {
    await access(p, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

const version =
  git("describe", "--tags", "--match", "v[0-9]*", "--always", "--dirty") ||
  "dev";
const commit = git("rev-parse", "--short", "HEAD") || "unknown";
const date = new Date().toISOString().replace(/\.\d+Z$/, "Z");
const ldflags = `-X main.version=${version} -X main.commit=${commit} -X main.date=${date}`;

console.log(
  `[bundle-cli] go build → ${srcBinary} (${goos}/${goarch}, version=${version} commit=${commit})`,
);
await mkdir(join(serverDir, "bin", `${goos}-${goarch}`), { recursive: true });
execFileSync(
  "go",
  [
    "build",
    "-ldflags",
    ldflags,
    "-o",
    srcBinary,
    "./cmd/patchbay",
  ],
  {
    cwd: serverDir,
    stdio: "inherit",
    env: {
      ...process.env,
      CGO_ENABLED: "0",
      GOOS: goos,
      GOARCH: goarch,
    },
  },
);

if (!(await exists(srcBinary))) {
  throw new Error(`[bundle-cli] Go did not produce ${srcBinary}`);
}

await rm(destDir, { recursive: true, force: true });
await mkdir(destDir, { recursive: true });
await copyFile(srcBinary, destBinary);
await chmod(destBinary, 0o755);
const digest = createHash("sha256")
  .update(await readFile(destBinary))
  .digest("hex");
await writeFile(
  join(destDir, `${binName}.sha256`),
  `${digest}  ${binName}\n`,
  { mode: 0o644 },
);

// macOS: ad-hoc sign so Gatekeeper doesn't complain when the parent app
// (which itself may be unsigned in dev) spawns the child.
if (process.platform === "darwin") {
  try {
    execSync(`codesign -s - --force ${JSON.stringify(destBinary)}`, {
      stdio: "pipe",
    });
  } catch {
    // Non-fatal. Unsigned binaries still run when the parent app is trusted.
  }
}

console.log(`[bundle-cli] bundled ${srcBinary} → ${destBinary}`);
