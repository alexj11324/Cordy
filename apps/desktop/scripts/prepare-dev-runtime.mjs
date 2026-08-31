#!/usr/bin/env node

import { access, chmod, copyFile, mkdir, rm } from "node:fs/promises";
import { constants } from "node:fs";
import { execFileSync } from "node:child_process";
import { delimiter, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  binaryNameForPlatform,
  cargoTargetDirectoryForProfile,
  devRustTargetFor,
  devBuildVariables,
  resolveCargoCommand,
  rustBuildEnvironment,
} from "./bundle-cli.mjs";
import {
  defaultDevCliCacheDir,
  pruneDevRuntimeCache,
  rustBuildEnvironmentFingerprint,
  rustSourceFingerprint,
  rustToolchainIdentity,
  stageCachedDevCli,
  storeDevCli,
} from "./dev-cli-cache.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..", "..", "..");

function executableName(name, platform) {
  return platform === "win32" ? `${name}.exe` : name;
}

export function devRuntimeComponents({
  repoRoot = defaultRepoRoot,
  platform = process.platform,
  arch = process.arch,
  cargoTargetDir = cargoTargetDirectoryForProfile(
    "dev",
    process.env,
    join(repoRoot, "server-rs"),
  ),
} = {}) {
  const rustTarget = devRustTargetFor(platform, arch);
  const builtDir = join(cargoTargetDir, rustTarget, "debug");
  const stagedDir = join(repoRoot, ".patchbay-dev", "bin");
  return [
    {
      id: "cli",
      packageName: "patchbay-cli",
      profile: "dev",
      binaryName: binaryNameForPlatform(platform),
      sourceBinary: join(builtDir, binaryNameForPlatform(platform)),
      destinationBinary: join(stagedDir, binaryNameForPlatform(platform)),
      additionalDestinations: [
        join(
        repoRoot,
        "apps",
        "desktop",
        "resources",
        "bin",
        binaryNameForPlatform(platform),
        ),
      ],
    },
    {
      id: "backend",
      packageName: "patchbay-server",
      profile: "dev-server",
      binaryName: executableName("patchbay-server", platform),
      sourceBinary: join(builtDir, executableName("patchbay-server", platform)),
      destinationBinary: join(
        stagedDir,
        executableName("patchbay-server", platform),
      ),
    },
    {
      id: "migrations",
      packageName: "patchbay-migrate",
      profile: "dev-migrate",
      binaryName: executableName("patchbay-migrate", platform),
      sourceBinary: join(
        builtDir,
        executableName("patchbay-migrate", platform),
      ),
      destinationBinary: join(
        stagedDir,
        executableName("patchbay-migrate", platform),
      ),
    },
  ];
}

async function stageAdditionalDestinations(component, platform) {
  for (const destination of component.additionalDestinations || []) {
    await mkdir(dirname(destination), { recursive: true });
    await rm(destination, { force: true });
    await rm(`${destination}.dev-manifest.json`, { force: true });
    await copyFile(component.destinationBinary, destination);
    await copyFile(
      `${component.destinationBinary}.dev-manifest.json`,
      `${destination}.dev-manifest.json`,
    );
    if (platform !== "win32") await chmod(destination, 0o755);
  }
}

export function devRuntimeBuildArguments(rustTarget, components) {
  return [
    "build",
    "--locked",
    "--target",
    rustTarget,
    ...components.flatMap(({ packageName }) => ["-p", packageName]),
    "--bins",
  ];
}

export function devRuntimeBuildEnvironment(env, cargoCommand) {
  const buildEnv = rustBuildEnvironment(env);
  if (isAbsolute(cargoCommand)) {
    buildEnv.PATH = [dirname(cargoCommand), buildEnv.PATH]
      .filter(Boolean)
      .join(delimiter);
  }
  return buildEnv;
}

async function exists(path) {
  try {
    await access(path, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

async function signMacBinary(path, platform) {
  if (process.platform !== "darwin" || platform !== "darwin") return;
  try {
    execFileSync("codesign", ["-s", "-", "--force", path], {
      stdio: "pipe",
    });
  } catch {
    // Best effort. Unsigned Cargo outputs still run in local development.
  }
}

export async function prepareDevRuntime({
  repoRoot = defaultRepoRoot,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
} = {}) {
  const serverRsDir = join(repoRoot, "server-rs");
  const rustTarget = devRustTargetFor(platform, arch);
  const cargoTargetDir = cargoTargetDirectoryForProfile(
    "dev",
    env,
    serverRsDir,
  );
  const sourceFingerprint = rustSourceFingerprint(repoRoot);
  const cargoCommand = resolveCargoCommand(env, platform);
  const toolchainIdentity = rustToolchainIdentity(env, cargoCommand, {
    platform,
    cwd: serverRsDir,
  });
  const buildVariables = devBuildVariables(
    sourceFingerprint,
    rustBuildEnvironmentFingerprint(env, rustTarget, "dev"),
  );
  const cacheRoot = defaultDevCliCacheDir({ env, platform });
  const components = devRuntimeComponents({
    repoRoot,
    platform,
    arch,
    cargoTargetDir,
  });

  const cached = await Promise.all(
    components.map((component) =>
      stageCachedDevCli({
        cacheRoot,
        sourceFingerprint,
        rustTarget,
        profile: component.profile,
        toolchainIdentity,
        buildVariables,
        destinationBinary: component.destinationBinary,
      }),
    ),
  );
  if (cached.every(Boolean)) {
    for (const component of components) {
      await stageAdditionalDestinations(component, platform);
    }
    await pruneDevRuntimeCache({ cacheRoot });
    console.log(
      `[dev-runtime] source-matched cache hit ${sourceFingerprint.slice(0, 12)} (${components.map(({ id }) => id).join(", ")})`,
    );
    return { cacheHit: true, components, sourceFingerprint };
  }

  if (!cargoCommand) {
    throw new Error(
      "[dev-runtime] cache miss requires Rust/Cargo; install Rust or set CARGO to its executable path",
    );
  }
  const buildEnv = devRuntimeBuildEnvironment(env, cargoCommand);
  console.log(
    `[dev-runtime] cache miss ${sourceFingerprint.slice(0, 12)}; building CLI, backend and migrations once${buildEnv.RUSTC_WRAPPER ? ` with ${buildEnv.RUSTC_WRAPPER}` : ""}`,
  );
  execFileSync(cargoCommand, devRuntimeBuildArguments(rustTarget, components), {
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

  for (const component of components) {
    if (!(await exists(component.sourceBinary))) {
      throw new Error(
        `[dev-runtime] Cargo did not produce ${component.sourceBinary}`,
      );
    }
    await mkdir(dirname(component.destinationBinary), { recursive: true });
    await rm(component.destinationBinary, { force: true });
    await copyFile(component.sourceBinary, component.destinationBinary);
    if (platform !== "win32") await chmod(component.destinationBinary, 0o755);
    await signMacBinary(component.destinationBinary, platform);
    await storeDevCli({
      cacheRoot,
      sourceBinary: component.destinationBinary,
      binaryName: component.binaryName,
      sourceFingerprint,
      rustTarget,
      profile: component.profile,
      toolchainIdentity,
      buildVariables,
      buildMetadata: { component: component.id },
    });
    await stageCachedDevCli({
      cacheRoot,
      sourceFingerprint,
      rustTarget,
      profile: component.profile,
      toolchainIdentity,
      buildVariables,
      destinationBinary: component.destinationBinary,
    });
    await stageAdditionalDestinations(component, platform);
  }

  await pruneDevRuntimeCache({ cacheRoot });

  return { cacheHit: false, components, sourceFingerprint };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  prepareDevRuntime().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
