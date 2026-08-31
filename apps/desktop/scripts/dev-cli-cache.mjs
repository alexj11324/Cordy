import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  createReadStream,
  lstatSync,
  readFileSync,
  readlinkSync,
} from "node:fs";
import {
  chmod,
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  utimes,
  writeFile,
} from "node:fs/promises";
import { homedir } from "node:os";
import {
  basename,
  delimiter,
  dirname,
  isAbsolute,
  join,
  resolve,
} from "node:path";

export const DEV_CLI_CACHE_SCHEMA_VERSION = 1;
export const DEV_RUNTIME_CACHE_MAX_BYTES = 5 * 1024 * 1024 * 1024;
export const DEV_RUNTIME_CACHE_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;
export const DEV_RUNTIME_CACHE_MIN_FINGERPRINTS = 10;
const COMPLETE_RUNTIME_PROFILES = new Set([
  "dev",
  "dev-server",
  "dev-migrate",
]);

function hashText(value) {
  return createHash("sha256").update(value).digest("hex");
}

const RUST_BUILD_ENV_KEYS = new Set([
  "AR",
  "CC",
  "CFLAGS",
  "CPPFLAGS",
  "CXX",
  "CXXFLAGS",
  "CARGO_BUILD_RUSTC",
  "CARGO_BUILD_RUSTC_WRAPPER",
  "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
  "CARGO_BUILD_RUSTDOC",
  "CARGO_BUILD_RUSTDOCFLAGS",
  "CARGO_BUILD_RUSTFLAGS",
  "HOST_AR",
  "HOST_CC",
  "HOST_CFLAGS",
  "HOST_CXX",
  "HOST_CXXFLAGS",
  "LDFLAGS",
  "MACOSX_DEPLOYMENT_TARGET",
  "RUSTC",
  "RUSTC_BOOTSTRAP",
  "RUSTC_WRAPPER",
  "RUSTC_WORKSPACE_WRAPPER",
  "RUSTDOCFLAGS",
  "RUSTFLAGS",
  "CARGO_ENCODED_RUSTFLAGS",
  "SDKROOT",
  "TARGET_AR",
  "TARGET_CC",
  "TARGET_CFLAGS",
  "TARGET_CXX",
  "TARGET_CXXFLAGS",
]);

const TARGET_TOOL_PREFIXES = [
  "AR",
  "CC",
  "CFLAGS",
  "CPPFLAGS",
  "CXX",
  "CXXFLAGS",
  "LDFLAGS",
];

export function rustBuildEnvironmentFingerprint(
  env = process.env,
  rustTarget = "",
  profile = "dev",
) {
  const normalizedTarget = rustTarget.toUpperCase().replace(/[^A-Z0-9]/g, "_");
  const lowercaseTarget = normalizedTarget.toLowerCase();
  const profilePrefix = `CARGO_PROFILE_${profile.toUpperCase()}_`;
  const targetSpecificKeys = new Set([
    `CARGO_TARGET_${normalizedTarget}_LINKER`,
    `CARGO_TARGET_${normalizedTarget}_RUSTFLAGS`,
    `CARGO_TARGET_${normalizedTarget}_RUSTDOCFLAGS`,
  ]);

  for (const prefix of TARGET_TOOL_PREFIXES) {
    targetSpecificKeys.add(`${prefix}_${rustTarget}`);
    targetSpecificKeys.add(`${prefix}_${normalizedTarget}`);
    targetSpecificKeys.add(`${prefix}_${lowercaseTarget}`);
    targetSpecificKeys.add(`${normalizedTarget}_${prefix}`);
    targetSpecificKeys.add(`${lowercaseTarget}_${prefix}`);
  }

  const entries = Object.entries(env)
    .filter(
      ([key, value]) =>
        value !== undefined &&
        value !== null &&
        (RUST_BUILD_ENV_KEYS.has(key) ||
          targetSpecificKeys.has(key) ||
          key.startsWith(profilePrefix)),
    )
    .map(([key, value]) => [key, String(value)])
    .sort(([left], [right]) => left.localeCompare(right));

  // Only the digest is persisted in manifests so compiler paths and flags do
  // not leak into a shared cache while still invalidating incompatible builds.
  return hashText(JSON.stringify(entries));
}

async function hashFile(filePath) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(filePath)) hash.update(chunk);
  return hash.digest("hex");
}

export function defaultDevCliCacheDir({
  env = process.env,
  platform = process.platform,
  home = homedir(),
} = {}) {
  const configured =
    env.PATCHBAY_DEV_RUNTIME_CACHE_DIR || env.PATCHBAY_DEV_CLI_CACHE_DIR;
  if (configured) {
    return resolve(configured);
  }
  if (platform === "darwin") {
    return join(home, "Library", "Caches", "Patchbay", "dev-runtime");
  }
  if (platform === "win32") {
    return join(
      env.LOCALAPPDATA || join(home, "AppData", "Local"),
      "Patchbay",
      "dev-runtime",
    );
  }
  return join(
    env.XDG_CACHE_HOME || join(home, ".cache"),
    "patchbay",
    "dev-runtime",
  );
}

export function listRustFingerprintFiles(repoRoot) {
  const output = execFileSync(
    "git",
    [
      "ls-files",
      "-z",
      "--cached",
      "--others",
      "--exclude-standard",
      "--",
      "server-rs",
      "rust-toolchain",
      "rust-toolchain.toml",
    ],
    { cwd: repoRoot },
  );
  return output.toString("utf8").split("\0").filter(Boolean).sort();
}

export function fingerprintRustFiles(repoRoot, files) {
  const hash = createHash("sha256");
  hash.update(`patchbay-dev-cli-source-v${DEV_CLI_CACHE_SCHEMA_VERSION}\0`);

  for (const relativePath of [...files].sort()) {
    const absolutePath = join(repoRoot, relativePath);
    hash.update(relativePath);
    hash.update("\0");
    try {
      const fileStat = lstatSync(absolutePath);
      if (fileStat.isSymbolicLink()) {
        hash.update("symlink\0");
        hash.update(readlinkSync(absolutePath));
      } else {
        hash.update("file\0");
        hash.update(readFileSync(absolutePath));
      }
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
      // A tracked deletion still appears in git ls-files. Recording it keeps
      // the fingerprint different from the last checkout that had the file.
      hash.update("missing");
    }
    hash.update("\0");
  }

  return hash.digest("hex");
}

export function rustSourceFingerprint(repoRoot) {
  return fingerprintRustFiles(repoRoot, listRustFingerprintFiles(repoRoot));
}

export function rustToolchainIdentity(
  env = process.env,
  cargoCommand,
  { platform = process.platform, execFile = execFileSync, cwd } = {},
) {
  if (!cargoCommand) return null;

  const executable = platform === "win32" ? "rustc.exe" : "rustc";
  const toolchainEnv = { ...env };
  if (isAbsolute(cargoCommand)) {
    toolchainEnv.PATH = [dirname(cargoCommand), toolchainEnv.PATH]
      .filter(Boolean)
      .join(delimiter);
  }

  let cargoIdentity;
  try {
    cargoIdentity = execFile(cargoCommand, ["-vV"], {
      encoding: "utf8",
      cwd,
      env: toolchainEnv,
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null;
  }

  const candidates = [env.RUSTC, executable].filter(Boolean);
  for (const candidate of [...new Set(candidates)]) {
    try {
      const rustcIdentity = execFile(candidate, ["-vV"], {
        encoding: "utf8",
        cwd,
        env: toolchainEnv,
        stdio: ["ignore", "pipe", "ignore"],
      }).trim();
      return `cargo:\n${cargoIdentity}\nrustc:\n${rustcIdentity}`;
    } catch {
      // Try the next compiler Cargo could resolve in the same environment.
    }
  }
  return null;
}

export function devCliCacheKey({
  sourceFingerprint,
  rustTarget,
  profile = "dev",
  toolchainIdentity,
  buildVariables = {},
}) {
  return hashText(
    JSON.stringify({
      schemaVersion: DEV_CLI_CACHE_SCHEMA_VERSION,
      sourceFingerprint,
      rustTarget,
      profile,
      toolchainIdentity: toolchainIdentity || "unavailable",
      buildVariables,
    }),
  );
}

function cacheProfileDir(cacheRoot, rustTarget, profile) {
  return join(
    cacheRoot,
    `v${DEV_CLI_CACHE_SCHEMA_VERSION}`,
    rustTarget,
    profile,
  );
}

async function readValidEntry(entryDir, expected) {
  try {
    const manifest = JSON.parse(
      await readFile(join(entryDir, "manifest.json"), "utf8"),
    );
    if (
      manifest.schemaVersion !== DEV_CLI_CACHE_SCHEMA_VERSION ||
      manifest.sourceFingerprint !== expected.sourceFingerprint ||
      manifest.rustTarget !== expected.rustTarget ||
      manifest.profile !== expected.profile ||
      (expected.toolchainIdentity &&
        manifest.toolchainIdentity !== expected.toolchainIdentity) ||
      (expected.cacheIdentityKey &&
        runtimeIdentityKey({
          sourceFingerprint: manifest.sourceFingerprint,
          rustTarget: manifest.rustTarget,
          toolchainIdentity: manifest.toolchainIdentity,
          buildVariables: manifest.buildVariables || {},
        }) !== expected.cacheIdentityKey) ||
      JSON.stringify(manifest.buildVariables || {}) !==
        JSON.stringify(expected.buildVariables || {})
    ) {
      return null;
    }

    const binaryPath = join(entryDir, manifest.binaryName);
    if ((await hashFile(binaryPath)) !== manifest.sha256) return null;
    return { binaryPath, entryDir, manifest };
  } catch {
    return null;
  }
}

export async function findCachedDevCli({
  cacheRoot,
  sourceFingerprint,
  rustTarget,
  profile = "dev",
  toolchainIdentity,
  cacheIdentityKey,
  buildVariables = {},
}) {
  const profileDir = cacheProfileDir(cacheRoot, rustTarget, profile);
  const expected = {
    sourceFingerprint,
    rustTarget,
    profile,
    toolchainIdentity,
    cacheIdentityKey,
    buildVariables,
  };

  if (toolchainIdentity) {
    const exactKey = devCliCacheKey(expected);
    return readValidEntry(join(profileDir, exactKey), expected);
  }

  // A compiler is not required to run an already-built CLI. When rustc is not
  // installed, select the newest checksum-valid entry for the exact source,
  // target and profile instead of making frontend-only contributors compile.
  let names;
  try {
    names = await readdir(profileDir);
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }

  const matches = (
    await Promise.all(
      names.map((name) => readValidEntry(join(profileDir, name), expected)),
    )
  ).filter(Boolean);
  matches.sort((left, right) =>
    right.manifest.createdAt.localeCompare(left.manifest.createdAt),
  );
  return matches[0] ?? null;
}

export async function stageCachedDevCli(options) {
  const cached = await findCachedDevCli(options);
  if (!cached) return null;

  await rm(options.destinationBinary, { force: true });
  await rm(`${options.destinationBinary}.dev-manifest.json`, { force: true });
  await mkdir(dirname(options.destinationBinary), { recursive: true });
  await copyFile(cached.binaryPath, options.destinationBinary);
  await copyFile(
    join(cached.entryDir, "manifest.json"),
    `${options.destinationBinary}.dev-manifest.json`,
  );
  if (process.platform !== "win32") {
    await chmod(options.destinationBinary, 0o755);
  }
  const now = new Date();
  await utimes(cached.entryDir, now, now).catch(() => {});
  return cached;
}

export async function storeDevCli({
  cacheRoot,
  sourceBinary,
  binaryName = basename(sourceBinary),
  sourceFingerprint,
  rustTarget,
  profile = "dev",
  toolchainIdentity,
  buildVariables = {},
  buildMetadata = {},
}) {
  const identity = {
    sourceFingerprint,
    rustTarget,
    profile,
    toolchainIdentity,
    buildVariables,
  };
  const key = devCliCacheKey(identity);
  const profileDir = cacheProfileDir(cacheRoot, rustTarget, profile);
  const entryDir = join(profileDir, key);
  const existing = await readValidEntry(entryDir, identity);
  if (existing) return existing;

  await mkdir(profileDir, { recursive: true });
  const temporaryDir = await mkdtemp(join(profileDir, `.${key}.tmp-`));
  const cachedBinary = join(temporaryDir, binaryName);
  await copyFile(sourceBinary, cachedBinary);
  if (process.platform !== "win32") await chmod(cachedBinary, 0o755);
  const manifest = {
    schemaVersion: DEV_CLI_CACHE_SCHEMA_VERSION,
    cacheKey: key,
    sourceFingerprint,
    rustTarget,
    profile,
    toolchainIdentity: toolchainIdentity || "unavailable",
    buildVariables,
    binaryName,
    sha256: await hashFile(cachedBinary),
    createdAt: new Date().toISOString(),
    ...buildMetadata,
  };
  await writeFile(
    join(temporaryDir, "manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );

  try {
    await rename(temporaryDir, entryDir);
  } catch (error) {
    if (error?.code !== "EEXIST" && error?.code !== "ENOTEMPTY") {
      await rm(temporaryDir, { recursive: true, force: true });
      throw error;
    }
    const winner = await readValidEntry(entryDir, identity);
    if (winner) {
      await rm(temporaryDir, { recursive: true, force: true });
      return winner;
    }
    // Only replace an existing entry after proving it is invalid. This avoids
    // two worktrees deleting each other's successful concurrent write.
    await rm(entryDir, { recursive: true, force: true });
    await rename(temporaryDir, entryDir);
  }
  return readValidEntry(entryDir, identity);
}

export async function pruneDevCliCache({
  cacheRoot,
  rustTarget,
  profile = "dev",
  keep = 5,
  preserveEntryDir,
}) {
  const profileDir = cacheProfileDir(cacheRoot, rustTarget, profile);
  let names;
  try {
    names = await readdir(profileDir);
  } catch (error) {
    if (error?.code === "ENOENT") return;
    throw error;
  }

  const entries = [];
  for (const name of names) {
    if (name.startsWith(".")) continue;
    const entryDir = join(profileDir, name);
    try {
      entries.push({ entryDir, mtimeMs: (await stat(entryDir)).mtimeMs });
    } catch {
      // Another process may be replacing an entry. It owns that cleanup.
    }
  }
  entries.sort((left, right) => right.mtimeMs - left.mtimeMs);
  const retained = new Set(
    entries.slice(0, keep).map(({ entryDir }) => entryDir),
  );
  if (preserveEntryDir) retained.add(preserveEntryDir);
  await Promise.all(
    entries
      .filter(({ entryDir }) => !retained.has(entryDir))
      .map(({ entryDir }) => rm(entryDir, { recursive: true, force: true })),
  );
}

async function directorySize(path) {
  let total = 0;
  const names = await readdir(path, { withFileTypes: true });
  for (const name of names) {
    const child = join(path, name.name);
    if (name.isDirectory()) total += await directorySize(child);
    else total += (await stat(child)).size;
  }
  return total;
}

function runtimeIdentityKey({
  sourceFingerprint,
  rustTarget,
  toolchainIdentity,
  buildVariables,
}) {
  return JSON.stringify({
    sourceFingerprint,
    rustTarget,
    toolchainIdentity: toolchainIdentity || "unavailable",
    buildVariables: buildVariables || {},
  });
}

async function listRuntimeCacheEntries(cacheRoot) {
  const schemaRoot = join(
    cacheRoot,
    `v${DEV_CLI_CACHE_SCHEMA_VERSION}`,
  );
  const entries = [];
  let targets;
  try {
    targets = await readdir(schemaRoot, { withFileTypes: true });
  } catch (error) {
    if (error?.code === "ENOENT") return entries;
    throw error;
  }
  for (const target of targets) {
    if (!target.isDirectory()) continue;
    const targetDir = join(schemaRoot, target.name);
    const profiles = await readdir(targetDir, { withFileTypes: true });
    for (const profile of profiles) {
      if (!profile.isDirectory()) continue;
      const profileDir = join(targetDir, profile.name);
      const names = await readdir(profileDir, { withFileTypes: true });
      for (const name of names) {
        if (!name.isDirectory() || name.name.startsWith(".")) continue;
        const entryDir = join(profileDir, name.name);
        try {
          const entryStat = await stat(entryDir);
          const manifest = JSON.parse(
            await readFile(join(entryDir, "manifest.json"), "utf8"),
          );
          const binaryPath = join(entryDir, manifest.binaryName);
          await stat(binaryPath);
          const toolchainIdentity = manifest.toolchainIdentity || "unavailable";
          const buildVariables = manifest.buildVariables || {};
          entries.push({
            entryDir,
            profile: profile.name,
            rustTarget: target.name,
            sourceFingerprint: manifest.sourceFingerprint,
            toolchainIdentity,
            buildVariables,
            identityKey: runtimeIdentityKey({
              sourceFingerprint: manifest.sourceFingerprint,
              rustTarget: target.name,
              toolchainIdentity,
              buildVariables,
            }),
            mtimeMs: entryStat.mtimeMs,
            sizeBytes: await directorySize(entryDir),
          });
        } catch {
          entries.push({
            entryDir,
            profile: profile.name,
            rustTarget: target.name,
            sourceFingerprint: null,
            toolchainIdentity: null,
            buildVariables: null,
            identityKey: null,
            mtimeMs: 0,
            sizeBytes: await directorySize(entryDir).catch(() => 0),
            invalid: true,
          });
        }
      }
    }
  }
  return entries;
}

export async function inspectDevRuntimeCache({ cacheRoot }) {
  const entries = await listRuntimeCacheEntries(cacheRoot);
  const fingerprints = new Map();
  for (const entry of entries) {
    if (!entry.sourceFingerprint || !entry.identityKey) continue;
    const key = entry.identityKey;
    const group = fingerprints.get(key) || {
      identityKey: key,
      sourceFingerprint: entry.sourceFingerprint,
      rustTarget: entry.rustTarget,
      toolchainIdentity: entry.toolchainIdentity,
      buildVariables: entry.buildVariables,
      profiles: new Set(),
      newestMtimeMs: 0,
      sizeBytes: 0,
    };
    group.profiles.add(entry.profile);
    group.newestMtimeMs = Math.max(group.newestMtimeMs, entry.mtimeMs);
    group.sizeBytes += entry.sizeBytes;
    fingerprints.set(key, group);
  }
  const completeFingerprints = [...fingerprints.values()].filter((group) =>
    [...COMPLETE_RUNTIME_PROFILES].every((profile) =>
      group.profiles.has(profile),
    ),
  );
  return {
    entries,
    entryCount: entries.length,
    totalBytes: entries.reduce((total, entry) => total + entry.sizeBytes, 0),
    completeFingerprintCount: completeFingerprints.length,
    completeFingerprints,
  };
}

export async function pruneDevRuntimeCache({
  cacheRoot,
  maxBytes = DEV_RUNTIME_CACHE_MAX_BYTES,
  maxAgeMs = DEV_RUNTIME_CACHE_MAX_AGE_MS,
  minFingerprints = DEV_RUNTIME_CACHE_MIN_FINGERPRINTS,
  nowMs = Date.now(),
  dryRun = false,
} = {}) {
  const report = await inspectDevRuntimeCache({ cacheRoot });
  const protectedGroups = [...report.completeFingerprints]
    .sort((left, right) => right.newestMtimeMs - left.newestMtimeMs)
    .slice(0, minFingerprints);
  const protectedEntries = new Set();
  for (const group of protectedGroups) {
    for (const entry of report.entries) {
      if (entry.identityKey === group.identityKey) {
        protectedEntries.add(entry.entryDir);
      }
    }
  }

  const removable = report.entries
    .filter((entry) => !protectedEntries.has(entry.entryDir))
    .sort((left, right) => left.mtimeMs - right.mtimeMs);
  const selected = new Set(
    removable
      .filter(
        (entry) => entry.invalid || nowMs - entry.mtimeMs > maxAgeMs,
      )
      .map((entry) => entry.entryDir),
  );
  let remainingBytes = report.totalBytes;
  for (const entry of removable) {
    if (selected.has(entry.entryDir)) remainingBytes -= entry.sizeBytes;
  }
  for (const entry of removable) {
    if (remainingBytes <= maxBytes) break;
    if (selected.has(entry.entryDir)) continue;
    selected.add(entry.entryDir);
    remainingBytes -= entry.sizeBytes;
  }
  if (!dryRun) {
    await Promise.all(
      [...selected].map((entryDir) =>
        rm(entryDir, { recursive: true, force: true }),
      ),
    );
  }
  return {
    ...report,
    removedCount: selected.size,
    removedBytes: report.totalBytes - remainingBytes,
    remainingBytes,
    protectedFingerprintCount: protectedGroups.length,
    dryRun,
  };
}
