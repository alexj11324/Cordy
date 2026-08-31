import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  defaultDevCliCacheDir,
  devCliCacheKey,
  findCachedDevCli,
  fingerprintRustFiles,
  stageCachedDevCli,
  storeDevCli,
} from "./dev-cli-cache.mjs";

let sandbox;

afterEach(async () => {
  if (sandbox) await rm(sandbox, { recursive: true, force: true });
  sandbox = undefined;
});

async function createSandbox() {
  sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-cli-cache-"));
  return sandbox;
}

describe("development CLI artifact cache", () => {
  it("keeps the cache user-global while leaving worktree targets independent", () => {
    expect(
      defaultDevCliCacheDir({
        platform: "darwin",
        home: "/Users/dev",
        env: {},
      }),
    ).toBe("/Users/dev/Library/Caches/Patchbay/dev-runtime");
    expect(
      defaultDevCliCacheDir({
        platform: "linux",
        home: "/home/dev",
        env: { XDG_CACHE_HOME: "/cache" },
      }),
    ).toBe("/cache/patchbay/dev-runtime");
  });

  it("changes the source fingerprint when tracked Rust content changes", async () => {
    const root = await createSandbox();
    const file = join(root, "server-rs-file");
    await writeFile(file, "first");
    const first = fingerprintRustFiles(root, ["server-rs-file"]);
    await writeFile(file, "second");
    const second = fingerprintRustFiles(root, ["server-rs-file"]);
    expect(second).not.toBe(first);
  });

  it("isolates cache keys by source, target, profile, toolchain and build variables", () => {
    const base = {
      sourceFingerprint: "source-a",
      rustTarget: "aarch64-apple-darwin",
      profile: "dev",
      toolchainIdentity: "rustc 1",
      buildVariables: { version: "dev-a" },
    };
    const key = devCliCacheKey(base);
    for (const changed of [
      { sourceFingerprint: "source-b" },
      { rustTarget: "x86_64-apple-darwin" },
      { profile: "release" },
      { toolchainIdentity: "rustc 2" },
      { buildVariables: { version: "dev-b" } },
    ]) {
      expect(devCliCacheKey({ ...base, ...changed })).not.toBe(key);
    }
  });

  it("stores, checksum-validates and stages an exact source artifact", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "patchbay-built");
    const destinationBinary = join(
      root,
      "worktree",
      "resources",
      "bin",
      "patchbay",
    );
    await writeFile(sourceBinary, "fixture CLI");
    await chmod(sourceBinary, 0o755);
    const identity = {
      cacheRoot,
      sourceFingerprint: "source-a",
      rustTarget: "aarch64-apple-darwin",
      profile: "dev",
      toolchainIdentity: "rustc 1",
      buildVariables: { version: "dev-source-a" },
    };

    await storeDevCli({ ...identity, sourceBinary, binaryName: "patchbay" });
    const cached = await stageCachedDevCli({ ...identity, destinationBinary });

    expect(cached).not.toBeNull();
    expect(await readFile(destinationBinary, "utf8")).toBe("fixture CLI");
    const manifest = JSON.parse(
      await readFile(`${destinationBinary}.dev-manifest.json`, "utf8"),
    );
    expect(manifest.sourceFingerprint).toBe("source-a");
    expect(manifest.toolchainIdentity).toBe("rustc 1");
  });

  it("rejects a corrupted artifact and a mismatched toolchain", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "patchbay-built");
    await writeFile(sourceBinary, "fixture CLI");
    const identity = {
      cacheRoot,
      sourceFingerprint: "source-a",
      rustTarget: "aarch64-apple-darwin",
      profile: "dev",
      toolchainIdentity: "rustc 1",
      buildVariables: {},
    };
    const stored = await storeDevCli({
      ...identity,
      sourceBinary,
      binaryName: "patchbay",
    });

    expect(
      await findCachedDevCli({ ...identity, toolchainIdentity: "rustc 2" }),
    ).toBeNull();
    await writeFile(stored.binaryPath, "tampered");
    expect(await findCachedDevCli(identity)).toBeNull();
  });

  it("can run a cached CLI without rustc while preserving exact source/target/profile", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "patchbay-built");
    await writeFile(sourceBinary, "fixture CLI");
    const identity = {
      cacheRoot,
      sourceFingerprint: "source-a",
      rustTarget: "aarch64-apple-darwin",
      profile: "dev",
      toolchainIdentity: "rustc 1",
      buildVariables: {},
    };
    await storeDevCli({ ...identity, sourceBinary, binaryName: "patchbay" });

    const cached = await findCachedDevCli({
      ...identity,
      toolchainIdentity: null,
    });
    expect(cached?.manifest.toolchainIdentity).toBe("rustc 1");
  });
});
