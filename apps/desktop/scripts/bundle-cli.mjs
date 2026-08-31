#!/usr/bin/env node
// Builds the Rust `patchbay` CLI from server-rs and copies the binary
// into apps/desktop/resources/bin/ so the complete Electron development
// environment and electron-builder (production packaging) pick it up.
//
// Build environment variables mirror `make build` so `patchbay --version`
// reports a meaningful version / commit / date.
//
// Development first checks a content-addressed per-user artifact cache. A
// cache miss requires Cargo and fails loudly; opening a UI that cannot drive
// the local daemon is not considered a successful development launch.

import { access, chmod, copyFile, mkdir, rm } from "node:fs/promises";
import { constants } from "node:fs";
import { execFileSync } from "node:child_process";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  defaultDevCliCacheDir,
  pruneDevCliCache,
  rustBuildEnvironmentFingerprint,
  rustSourceFingerprint,
  rustToolchainIdentity,
  stageCachedDevCli,
  storeDevCli,
} from "./dev-cli-cache.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const serverRsDir = join(repoRoot, "server-rs");

const PLATFORM_TO_OS = {
  darwin: "darwin",
  linux: "linux",
  win32: "windows",
};

const SUPPORTED_ARCHS = new Set(["x64", "arm64"]);
const BUILD_PROFILES = new Set(["dev", "release"]);

const RUST_TARGETS = {
  darwin: {
    x64: "x86_64-apple-darwin",
    arm64: "aarch64-apple-darwin",
  },
  linux: {
    // Keep the bundled CLI independent of the glibc version on the build
    // runner. The Electron host remains platform-specific, but this child
    // binary can run on older supported Linux distributions when statically
    // linked against musl.
    x64: "x86_64-unknown-linux-musl",
    arm64: "aarch64-unknown-linux-musl",
  },
  win32: {
    x64: "x86_64-pc-windows-msvc",
    arm64: "aarch64-pc-windows-msvc",
  },
};

// Development uses the host toolchain target so a fresh checkout only needs
// the documented stable Rust/Cargo installation. Release Linux binaries stay
// on musl above for distribution compatibility.
const DEV_RUST_TARGETS = {
  darwin: RUST_TARGETS.darwin,
  linux: {
    x64: "x86_64-unknown-linux-gnu",
    arm64: "aarch64-unknown-linux-gnu",
  },
  win32: RUST_TARGETS.win32,
};

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

export function normalizeRuntimePlatform(platform) {
  if (Object.hasOwn(PLATFORM_TO_OS, platform)) return platform;
  throw new Error(
    `[bundle-cli] unsupported target platform: ${platform}. ` +
      "Use darwin, linux, or win32.",
  );
}

export function normalizeRuntimeArch(arch) {
  if (SUPPORTED_ARCHS.has(arch)) return arch;
  throw new Error(
    `[bundle-cli] unsupported target architecture: ${arch}. ` +
      "Use x64 or arm64.",
  );
}

export function binaryNameForPlatform(platform) {
  return platform === "win32" ? "patchbay.exe" : "patchbay";
}

export function rustTargetFor(platform, arch) {
  const platformTargets = Object.hasOwn(RUST_TARGETS, platform)
    ? RUST_TARGETS[platform]
    : undefined;
  const target = platformTargets?.[arch];
  if (!target) {
    throw new Error(
      `[bundle-cli] no Rust target for ${platform}/${arch}. ` +
        "Use darwin, linux, or win32 with x64 or arm64.",
    );
  }
  return target;
}

export function devRustTargetFor(platform, arch) {
  const platformTargets = Object.hasOwn(DEV_RUST_TARGETS, platform)
    ? DEV_RUST_TARGETS[platform]
    : undefined;
  const target = platformTargets?.[arch];
  if (!target) {
    throw new Error(
      `[dev-runtime] no native Rust target for ${platform}/${arch}. ` +
        "Use darwin, linux, or win32 with x64 or arm64.",
    );
  }
  return target;
}

export function cargoTargetDirectory(env = process.env, cwd = serverRsDir) {
  const configured = env.CARGO_TARGET_DIR;
  return configured ? resolve(cwd, configured) : join(cwd, "target");
}

export function cargoTargetDirectoryForProfile(
  profile,
  env = process.env,
  cwd = serverRsDir,
) {
  // Development outputs belong to one checkout. Sharing target/ across
  // worktrees causes Cargo locks, stale incremental state and unsafe cleanup;
  // cross-worktree reuse happens only through sccache and the artifact cache.
  return profile === "dev"
    ? join(cwd, "target")
    : cargoTargetDirectory(env, cwd);
}

export function buildProfileFromArgs(argv) {
  const flagIndex = argv.indexOf("--profile");
  if (flagIndex === -1) {
    throw new Error(
      "[bundle-cli] an explicit --profile dev or --profile release is required",
    );
  }
  const profile = argv[flagIndex + 1] ?? "";
  if (BUILD_PROFILES.has(profile)) return profile;
  throw new Error(
    `[bundle-cli] unsupported build profile: ${profile}. Use dev or release.`,
  );
}

export function enforceCliAvailability(profile, available, detail) {
  if (!available) {
    throw new Error(`[bundle-cli] ${profile} CLI build is required: ${detail}`);
  }
  return available;
}

export function cargoProfileDirectory(profile) {
  return profile === "dev" ? "debug" : "release";
}

export function cargoBuildArguments(profile, rustTarget) {
  return [
    "build",
    ...(profile === "release" ? ["--release"] : []),
    "--locked",
    "-p",
    "patchbay-cli",
    "--target",
    rustTarget,
  ];
}

export function buildDateForProfile(profile, commitDate, now = new Date()) {
  if (profile === "dev") return commitDate || "unknown";
  return now.toISOString().replace(/\.\d+Z$/, "Z");
}

export function devBuildVariables(sourceFingerprint, environmentFingerprint) {
  const shortFingerprint = sourceFingerprint.slice(0, 12);
  const variables = {
    version: `dev-${shortFingerprint}`,
    commit: `source-${shortFingerprint}`,
    date: "source-matched-dev",
  };
  if (environmentFingerprint) {
    variables.environmentFingerprint = environmentFingerprint;
  }
  return variables;
}

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

export function resolveCargoCommand(
  env = process.env,
  platform = process.platform,
) {
  const executable = platform === "win32" ? "cargo.exe" : "cargo";
  const candidates = [
    env.CARGO,
    executable,
    join(homedir(), ".cargo", "bin", executable),
  ].filter(Boolean);
  for (const candidate of [...new Set(candidates)]) {
    try {
      execFileSync(candidate, ["--version"], { env, stdio: "pipe" });
      return candidate;
    } catch {
      // Try the next deterministic Cargo location.
    }
  }
  return null;
}

function hasSccache() {
  try {
    execFileSync("sccache", ["--version"], { stdio: "pipe" });
    return true;
  } catch {
    return false;
  }
}

export function rustBuildEnvironment(
  env = process.env,
  sccacheAvailable = hasSccache(),
) {
  if (
    env.RUSTC_WRAPPER ||
    env.PATCHBAY_DISABLE_SCCACHE === "1" ||
    !sccacheAvailable
  ) {
    return env;
  }
  return { ...env, RUSTC_WRAPPER: "sccache" };
}

async function exists(p) {
  try {
    await access(p, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

async function main() {
  const argv = process.argv.slice(2);
  const profile = buildProfileFromArgs(argv);
  const targetPlatform = normalizeRuntimePlatform(
    runtimePlatformFromArgs(argv),
  );
  const targetArch = normalizeRuntimeArch(runtimeArchFromArgs(argv));
  const targetOs = PLATFORM_TO_OS[targetPlatform];
  const targetArchLabel = targetArch === "x64" ? "amd64" : targetArch;
  const rustTarget = rustTargetFor(targetPlatform, targetArch);
  const binName = binaryNameForPlatform(targetPlatform);
  // Cargo resolves a relative CARGO_TARGET_DIR from its working directory.
  // Normalize it once and give the same absolute directory to both the build
  // and copy phases so a stale server-rs/target binary can never win.
  const cargoTargetDir = cargoTargetDirectoryForProfile(
    profile,
    process.env,
    serverRsDir,
  );
  const srcBinary = join(
    cargoTargetDir,
    rustTarget,
    cargoProfileDirectory(profile),
    binName,
  );
  const destDir = join(repoRoot, "apps", "desktop", "resources", "bin");
  const destBinary = join(destDir, binName);

  const cargoCommand = resolveCargoCommand(process.env);
  let devCache;
  if (profile === "dev") {
    const sourceFingerprint = rustSourceFingerprint(repoRoot);
    const toolchainIdentity = rustToolchainIdentity(process.env, cargoCommand, {
      cwd: serverRsDir,
    });
    const buildVariables = devBuildVariables(
      sourceFingerprint,
      rustBuildEnvironmentFingerprint(process.env, rustTarget, profile),
    );
    const cacheRoot = defaultDevCliCacheDir();
    const cached = await stageCachedDevCli({
      cacheRoot,
      sourceFingerprint,
      rustTarget,
      profile,
      toolchainIdentity,
      buildVariables,
      destinationBinary: destBinary,
    });
    if (cached) {
      console.log(
        `[bundle-cli] source-matched CLI cache hit ${sourceFingerprint.slice(0, 12)} → ${destBinary}`,
      );
      return;
    }
    devCache = {
      cacheRoot,
      sourceFingerprint,
      toolchainIdentity,
      buildVariables,
    };
  }

  enforceCliAvailability(
    profile,
    Boolean(cargoCommand),
    "Cargo is unavailable; install Rust or set CARGO to its executable path",
  );

  if (cargoCommand) {
    const releaseVersion =
      git("describe", "--tags", "--match", "v[0-9]*", "--always", "--dirty") ||
      "dev";
    const releaseCommit = git("rev-parse", "--short", "HEAD") || "unknown";
    // A wall-clock timestamp makes Cargo rerun patchbay-cli's build script on
    // every launch because build.rs watches PATCHBAY_BUILD_DATE. Development
    // uses the commit date instead: it stays stable across no-op restarts,
    // while Cargo still sees and recompiles actual Rust source changes.
    const releaseDate = buildDateForProfile(
      profile,
      git("show", "-s", "--format=%cI", "HEAD"),
    );
    const buildVariables = devCache?.buildVariables ?? {
      version: releaseVersion,
      commit: releaseCommit,
      date: releaseDate,
    };

    console.log(
      `[bundle-cli] cargo build (${profile}) → ${srcBinary} (${targetOs}/${targetArchLabel}, target=${rustTarget}, version=${buildVariables.version} commit=${buildVariables.commit})`,
    );
    const buildEnv = rustBuildEnvironment(process.env);
    if (buildEnv.RUSTC_WRAPPER && !process.env.RUSTC_WRAPPER) {
      console.log(
        `[bundle-cli] using shared compiler cache: ${buildEnv.RUSTC_WRAPPER}`,
      );
    }
    execFileSync(cargoCommand, cargoBuildArguments(profile, rustTarget), {
      cwd: serverRsDir,
      stdio: "inherit",
      env: {
        ...buildEnv,
        CARGO_TARGET_DIR: cargoTargetDir,
        PATCHBAY_BUILD_VERSION: buildVariables.version,
        PATCHBAY_BUILD_COMMIT: buildVariables.commit,
        PATCHBAY_BUILD_DATE: buildVariables.date,
        PATCHBAY_GIT_COMMIT: buildVariables.commit,
      },
    });
  }

  const sourceBinaryExists = await exists(srcBinary);
  enforceCliAvailability(
    profile,
    sourceBinaryExists,
    `${srcBinary} was not produced`,
  );
  if (!sourceBinaryExists) {
    console.warn(
      `[bundle-cli] ${srcBinary} not present — Desktop will fall back to ` +
        `auto-installing the latest release at runtime.`,
    );
    await rm(destDir, { recursive: true, force: true });
    return;
  }

  await rm(destDir, { recursive: true, force: true });
  await mkdir(destDir, { recursive: true });
  await copyFile(srcBinary, destBinary);
  await chmod(destBinary, 0o755);

  // macOS: ad-hoc sign a macOS child so Gatekeeper doesn't complain when the
  // parent app (which itself may be unsigned in dev) spawns it. A macOS host
  // can package Linux/Windows targets too, and those binaries are not
  // codesignable Mach-O objects.
  if (process.platform === "darwin" && targetPlatform === "darwin") {
    try {
      execFileSync("codesign", ["-s", "-", "--force", destBinary], {
        stdio: "pipe",
      });
    } catch {
      // Non-fatal. Unsigned binaries still run when the parent app is trusted.
    }
  }

  if (devCache) {
    const cached = await storeDevCli({
      ...devCache,
      sourceBinary: destBinary,
      binaryName: binName,
      rustTarget,
      profile,
      buildMetadata: {
        repositoryCommit: git("rev-parse", "HEAD") || "unknown",
      },
    });
    await stageCachedDevCli({
      ...devCache,
      rustTarget,
      profile,
      destinationBinary: destBinary,
    });
    await pruneDevCliCache({
      cacheRoot: devCache.cacheRoot,
      rustTarget,
      profile,
      keep: 5,
      preserveEntryDir: cached?.entryDir,
    });
    console.log(
      `[bundle-cli] cached source CLI ${devCache.sourceFingerprint.slice(0, 12)} in ${devCache.cacheRoot}`,
    );
  }

  console.log(`[bundle-cli] bundled ${srcBinary} → ${destBinary}`);
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
