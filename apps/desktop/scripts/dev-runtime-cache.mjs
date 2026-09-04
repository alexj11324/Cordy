import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { lstatSync, readFileSync, readlinkSync } from "node:fs";
import {
  chmod,
  copyFile,
  mkdir,
  readdir,
  readFile,
  rename,
  rm,
  stat,
  utimes,
  writeFile,
} from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

export const DEV_RUNTIME_CACHE_SCHEMA_VERSION = 1;

function hashText(value) {
  return createHash("sha256").update(value).digest("hex");
}

async function hashFile(filePath) {
  return createHash("sha256").update(await readFile(filePath)).digest("hex");
}

export function defaultDevRuntimeCacheDir({
  env = process.env,
  platform = process.platform,
  home = homedir(),
} = {}) {
  const configured = env.PATCHBAY_DEV_RUNTIME_CACHE_DIR;
  if (configured) return resolve(configured);
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

/**
 * Return every source and module file that can affect a Go runtime binary.
 * `git ls-files` includes tracked deletions and non-ignored untracked files;
 * ignored build outputs such as server/bin are intentionally excluded.
 */
export function listGoSourceFiles(repoRoot) {
  const output = execFileSync(
    "git",
    [
      "ls-files",
      "-z",
      "--cached",
      "--others",
      "--exclude-standard",
      "--",
      "server",
    ],
    { cwd: repoRoot },
  );
  return output.toString("utf8").split("\0").filter(Boolean).sort();
}

export function fingerprintFiles(repoRoot, files) {
  const hash = createHash("sha256");
  hash.update(`patchbay-go-runtime-v${DEV_RUNTIME_CACHE_SCHEMA_VERSION}\0`);

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
      // A tracked deletion must change the key instead of reusing an artifact
      // produced by the checkout that still had the file.
      hash.update("missing");
    }
    hash.update("\0");
  }
  return hash.digest("hex");
}

export function goSourceFingerprint(repoRoot, files = listGoSourceFiles(repoRoot)) {
  return fingerprintFiles(repoRoot, files);
}

export function goTargetFor(platform = process.platform, arch = process.arch) {
  const goos =
    platform === "darwin" ? "darwin" : platform === "win32" ? "windows" : platform;
  const goarch = arch === "x64" ? "amd64" : arch;
  if (!["darwin", "linux", "windows"].includes(goos)) {
    throw new Error(`[dev-runtime] unsupported target platform: ${platform}`);
  }
  if (!["amd64", "arm64"].includes(goarch)) {
    throw new Error(`[dev-runtime] unsupported target architecture: ${arch}`);
  }
  return {
    goos,
    goarch,
    target: `${goos}-${goarch}`,
    suffix: goos === "windows" ? ".exe" : "",
  };
}

export function goToolchainIdentity(env = process.env) {
  const candidates = [env.GO, "go", "go.exe"].filter(Boolean);
  for (const candidate of [...new Set(candidates)]) {
    try {
      return execFileSync(candidate, ["version"], {
        encoding: "utf8",
        env,
        stdio: ["ignore", "pipe", "ignore"],
      }).trim();
    } catch {
      // A cache hit may be used without a Go installation. Try the next path.
    }
  }
  return null;
}

export function devRuntimeCacheKey({
  sourceFingerprint,
  target,
  profile = "dev",
  toolchainIdentity,
  buildVariables = {},
}) {
  return hashText(
    JSON.stringify({
      schemaVersion: DEV_RUNTIME_CACHE_SCHEMA_VERSION,
      sourceFingerprint,
      target,
      profile,
      toolchainIdentity: toolchainIdentity || "unavailable",
      buildVariables,
    }),
  );
}

function profileDir(cacheRoot, target, profile) {
  return join(
    cacheRoot,
    `v${DEV_RUNTIME_CACHE_SCHEMA_VERSION}`,
    target,
    profile,
  );
}

async function readValidEntry(entryDir, expected) {
  try {
    const manifest = JSON.parse(
      await readFile(join(entryDir, "manifest.json"), "utf8"),
    );
    if (
      manifest.schemaVersion !== DEV_RUNTIME_CACHE_SCHEMA_VERSION ||
      manifest.sourceFingerprint !== expected.sourceFingerprint ||
      manifest.target !== expected.target ||
      manifest.profile !== expected.profile ||
      (expected.toolchainIdentity &&
        manifest.toolchainIdentity !== expected.toolchainIdentity) ||
      JSON.stringify(manifest.buildVariables || {}) !==
        JSON.stringify(expected.buildVariables || {}) ||
      typeof manifest.binaryName !== "string" ||
      basename(manifest.binaryName) !== manifest.binaryName
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

export async function findCachedDevRuntime({
  cacheRoot,
  sourceFingerprint,
  target,
  profile = "dev",
  toolchainIdentity,
  buildVariables = {},
}) {
  const directory = profileDir(cacheRoot, target, profile);
  const expected = {
    sourceFingerprint,
    target,
    profile,
    toolchainIdentity,
    buildVariables,
  };

  if (toolchainIdentity) {
    const exactKey = devRuntimeCacheKey(expected);
    return readValidEntry(join(directory, exactKey), expected);
  }

  let names;
  try {
    names = await readdir(directory);
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
  const matches = (
    await Promise.all(
      names
        .filter((name) => !name.startsWith("."))
        .map((name) => readValidEntry(join(directory, name), expected)),
    )
  ).filter(Boolean);
  matches.sort((left, right) =>
    right.manifest.createdAt.localeCompare(left.manifest.createdAt),
  );
  return matches[0] ?? null;
}

export async function stageCachedDevRuntime(options) {
  const cached = await findCachedDevRuntime(options);
  if (!cached) return null;
  await rm(options.destinationBinary, { force: true });
  await rm(`${options.destinationBinary}.dev-manifest.json`, { force: true });
  await mkdir(dirname(options.destinationBinary), { recursive: true });
  await copyFile(cached.binaryPath, options.destinationBinary);
  await copyFile(
    join(cached.entryDir, "manifest.json"),
    `${options.destinationBinary}.dev-manifest.json`,
  );
  // Local Guest and the daemon resolver verify the bundled executable through
  // the established sha256 sidecar used by packaged Desktop builds. Keep the
  // cache manifest private to the preparer, but emit that runtime contract at
  // the staged destination as well.
  await writeFile(
    `${options.destinationBinary}.sha256`,
    `${cached.manifest.sha256}  ${basename(options.destinationBinary)}\n`,
    { mode: 0o644 },
  );
  if (options.destinationBinary.endsWith(".exe") === false) {
    await chmod(options.destinationBinary, 0o755);
  }
  const now = new Date();
  await utimes(cached.entryDir, now, now).catch(() => {});
  return cached;
}

export async function storeDevRuntime({
  cacheRoot,
  sourceBinary,
  binaryName = basename(sourceBinary),
  sourceFingerprint,
  target,
  profile = "dev",
  toolchainIdentity,
  buildVariables = {},
  buildMetadata = {},
}) {
  const identity = {
    sourceFingerprint,
    target,
    profile,
    toolchainIdentity,
    buildVariables,
  };
  const key = devRuntimeCacheKey(identity);
  const directory = profileDir(cacheRoot, target, profile);
  const entryDir = join(directory, key);
  const existing = await readValidEntry(entryDir, identity);
  if (existing) return existing;

  await mkdir(directory, { recursive: true });
  const temporaryDir = join(directory, `.${key}.tmp-${process.pid}-${Date.now()}`);
  await mkdir(temporaryDir);
  const cachedBinary = join(temporaryDir, binaryName);
  await copyFile(sourceBinary, cachedBinary);
  if (!binaryName.endsWith(".exe")) await chmod(cachedBinary, 0o755);
  const manifest = {
    schemaVersion: DEV_RUNTIME_CACHE_SCHEMA_VERSION,
    cacheKey: key,
    sourceFingerprint,
    target,
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
    await rm(entryDir, { recursive: true, force: true });
    await rename(temporaryDir, entryDir);
  }
  return readValidEntry(entryDir, identity);
}

export async function pruneDevRuntimeCache({
  cacheRoot,
  target,
  profile = "dev",
  keep = 5,
  preserveEntryDir,
}) {
  const directory = profileDir(cacheRoot, target, profile);
  let names;
  try {
    names = await readdir(directory);
  } catch (error) {
    if (error?.code === "ENOENT") return;
    throw error;
  }
  const entries = [];
  for (const name of names) {
    if (name.startsWith(".")) continue;
    const entryDir = join(directory, name);
    try {
      entries.push({ entryDir, mtimeMs: (await stat(entryDir)).mtimeMs });
    } catch {
      // Another process may be replacing an entry.
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
