#!/usr/bin/env node

import { access, chmod, copyFile, mkdir, rm } from "node:fs/promises";
import { constants } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  defaultDevRuntimeCacheDir,
  goSourceFingerprint,
  goTargetFor,
  goToolchainIdentity,
  pruneDevRuntimeCache,
  stageCachedDevRuntime,
  storeDevRuntime,
} from "./dev-runtime-cache.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..", "..", "..");

function executableName(name, suffix) {
  return `${name}${suffix}`;
}

export function devRuntimeComponents({
  repoRoot = defaultRepoRoot,
  platform = process.platform,
  arch = process.arch,
} = {}) {
  const target = goTargetFor(platform, arch);
  const sourceDir = join(repoRoot, "server", "bin", target.target);
  const stagedDir = join(repoRoot, ".patchbay-dev", "bin");
  return [
    {
      id: "cli",
      packagePath: "./cmd/patchbay",
      profile: "dev-cli",
      binaryName: executableName("patchbay", target.suffix),
      sourceBinary: join(
        sourceDir,
        executableName("patchbay", target.suffix),
      ),
      destinationBinary: join(
        repoRoot,
        "apps",
        "desktop",
        "resources",
        "bin",
        executableName("patchbay", target.suffix),
      ),
    },
    {
      id: "backend",
      packagePath: "./cmd/server",
      profile: "dev-server",
      binaryName: executableName("server", target.suffix),
      sourceBinary: join(sourceDir, executableName("server", target.suffix)),
      destinationBinary: join(
        stagedDir,
        executableName("server", target.suffix),
      ),
    },
    {
      id: "migrations",
      packagePath: "./cmd/migrate",
      profile: "dev-migrate",
      binaryName: executableName("migrate", target.suffix),
      sourceBinary: join(sourceDir, executableName("migrate", target.suffix)),
      destinationBinary: join(
        stagedDir,
        executableName("migrate", target.suffix),
      ),
    },
  ];
}

export function goBuildArguments(component, buildVariables) {
  const ldflags = [];
  if (component.id === "cli") {
    ldflags.push(
      `-X main.version=${buildVariables.version}`,
      `-X main.commit=${buildVariables.commit}`,
      `-X main.date=${buildVariables.date}`,
    );
  } else if (component.id === "backend") {
    ldflags.push(
      `-X main.version=${buildVariables.version}`,
      `-X main.commit=${buildVariables.commit}`,
    );
  }
  return [
    "build",
    "-trimpath",
    ...(ldflags.length > 0 ? ["-ldflags", ldflags.join(" ")] : []),
    "-o",
    component.sourceBinary,
    component.packagePath,
  ];
}

function git(repoRoot, ...args) {
  try {
    return execFileSync("git", args, { encoding: "utf8", cwd: repoRoot }).trim();
  } catch {
    return "";
  }
}

function buildVariables(repoRoot) {
  return {
    version:
      git(repoRoot, "describe", "--tags", "--match", "v[0-9]*", "--always", "--dirty") ||
      "dev",
    commit: git(repoRoot, "rev-parse", "--short", "HEAD") || "unknown",
    date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
  };
}

async function exists(path) {
  try {
    await access(path, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

function goExecutable(env) {
  return env.GO || (process.platform === "win32" ? "go.exe" : "go");
}

export async function prepareDevRuntime({
  repoRoot = defaultRepoRoot,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
} = {}) {
  const target = goTargetFor(platform, arch);
  const components = devRuntimeComponents({ repoRoot, platform, arch });
  const sourceFingerprint = goSourceFingerprint(repoRoot);
  const toolchainIdentity = goToolchainIdentity(env);
  const variables = buildVariables(repoRoot);
  const cacheBuildVariables = {
    version: variables.version,
    commit: variables.commit,
    cgoEnabled: "0",
    goflags: env.GOFLAGS || "",
    trimpath: true,
  };
  const cacheRoot = defaultDevRuntimeCacheDir({ env, platform });

  const cached = await Promise.all(
    components.map((component) =>
      stageCachedDevRuntime({
        cacheRoot,
        sourceFingerprint,
        target: target.target,
        profile: component.profile,
        toolchainIdentity,
        buildVariables: cacheBuildVariables,
        destinationBinary: component.destinationBinary,
      }),
    ),
  );

  if (cached.every(Boolean)) {
    console.log(
      `[dev-runtime] source-matched cache hit ${sourceFingerprint.slice(0, 12)} (${components.map(({ id }) => id).join(", ")})`,
    );
    return {
      cacheHit: true,
      components,
      sourceFingerprint,
      cacheKey: cached.map((entry) => entry.manifest.cacheKey),
    };
  }

  const go = goExecutable(env);
  if (!toolchainIdentity) {
    throw new Error(
      "[dev-runtime] cache miss requires Go; install Go 1.26.6 or set GO to its executable path",
    );
  }

  const buildEnv = {
    ...env,
    CGO_ENABLED: "0",
    GOOS: target.goos,
    GOARCH: target.goarch,
  };
  console.log(
    `[dev-runtime] cache miss ${sourceFingerprint.slice(0, 12)}; building missing Go runtime artifacts for ${target.target}`,
  );

  for (let index = 0; index < components.length; index += 1) {
    if (cached[index]) continue;
    const component = components[index];
    await mkdir(dirname(component.sourceBinary), { recursive: true });
    execFileSync(
      go,
      goBuildArguments(component, variables),
      {
        cwd: join(repoRoot, "server"),
        stdio: "inherit",
        env: buildEnv,
      },
    );
    if (!(await exists(component.sourceBinary))) {
      throw new Error(
        `[dev-runtime] Go did not produce ${component.sourceBinary}`,
      );
    }
    await rm(component.destinationBinary, { force: true });
    await mkdir(dirname(component.destinationBinary), { recursive: true });
    await stageSourceBinary(component);
    const stored = await storeDevRuntime({
      cacheRoot,
      sourceBinary: component.destinationBinary,
      binaryName: component.binaryName,
      sourceFingerprint,
      target: target.target,
      profile: component.profile,
      toolchainIdentity,
      buildVariables: cacheBuildVariables,
      buildMetadata: { component: component.id },
    });
    await stageCachedDevRuntime({
      cacheRoot,
      sourceFingerprint,
      target: target.target,
      profile: component.profile,
      toolchainIdentity,
      buildVariables: cacheBuildVariables,
      destinationBinary: component.destinationBinary,
    });
    await pruneDevRuntimeCache({
      cacheRoot,
      target: target.target,
      profile: component.profile,
      keep: 5,
      preserveEntryDir: stored?.entryDir,
    });
  }

  return { cacheHit: false, components, sourceFingerprint };
}

async function stageSourceBinary(component) {
  await copyFile(component.sourceBinary, component.destinationBinary);
  if (!component.binaryName.endsWith(".exe")) {
    await chmod(component.destinationBinary, 0o755);
  }
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
