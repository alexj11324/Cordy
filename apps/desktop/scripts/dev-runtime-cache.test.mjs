// @vitest-environment node
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  defaultDevRuntimeCacheDir,
  devRuntimeCacheKey,
  findCachedDevRuntime,
  fingerprintFiles,
  goTargetFor,
  stageCachedDevRuntime,
  storeDevRuntime,
} from "./dev-runtime-cache.mjs";

let sandbox;

afterEach(async () => {
  if (sandbox) await rm(sandbox, { recursive: true, force: true });
  sandbox = undefined;
});

async function createSandbox() {
  sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-runtime-"));
  return sandbox;
}

describe("Go development runtime cache", () => {
  it("keeps the cache user-global and platform-specific", () => {
    expect(
      defaultDevRuntimeCacheDir({
        platform: "darwin",
        home: "/Users/dev",
        env: {},
      }),
    ).toBe("/Users/dev/Library/Caches/Patchbay/dev-runtime");
    expect(
      defaultDevRuntimeCacheDir({
        platform: "linux",
        home: "/home/dev",
        env: { XDG_CACHE_HOME: "/cache" },
      }),
    ).toBe("/cache/patchbay/dev-runtime");
  });

  it("normalizes the three supported Go targets", () => {
    expect(goTargetFor("darwin", "arm64")).toEqual({
      goos: "darwin",
      goarch: "arm64",
      target: "darwin-arm64",
      suffix: "",
    });
    expect(goTargetFor("win32", "x64")).toEqual({
      goos: "windows",
      goarch: "amd64",
      target: "windows-amd64",
      suffix: ".exe",
    });
    expect(() => goTargetFor("freebsd", "x64")).toThrow(/unsupported target platform/i);
  });

  it("changes the fingerprint when Go source content changes", async () => {
    const root = await createSandbox();
    const file = join(root, "server", "main.go");
    await mkdir(join(root, "server"), { recursive: true });
    await writeFile(file, "package main\n");
    const first = fingerprintFiles(root, ["server/main.go"]);
    await writeFile(file, "package main\nvar changed = true\n");
    const second = fingerprintFiles(root, ["server/main.go"]);
    expect(second).not.toBe(first);
  });

  it("isolates cache keys by source, target, profile, toolchain and build variables", () => {
    const base = {
      sourceFingerprint: "source-a",
      target: "darwin-arm64",
      profile: "dev-cli",
      toolchainIdentity: "go1.26.6",
      buildVariables: { commit: "abc123", cgoEnabled: "0" },
    };
    const key = devRuntimeCacheKey(base);
    for (const changed of [
      { sourceFingerprint: "source-b" },
      { target: "windows-amd64" },
      { profile: "dev-server" },
      { toolchainIdentity: "go1.27.0" },
      { buildVariables: { commit: "def456", cgoEnabled: "0" } },
    ]) {
      expect(devRuntimeCacheKey({ ...base, ...changed })).not.toBe(key);
    }
  });

  it("stores, checksum-validates and stages an exact source artifact", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "built", "patchbay");
    const destinationBinary = join(root, "worktree", "bin", "patchbay");
    await mkdir(join(root, "built"), { recursive: true });
    await writeFile(sourceBinary, "fixture Go CLI");
    await chmod(sourceBinary, 0o755);
    const identity = {
      cacheRoot,
      sourceFingerprint: "source-a",
      target: "darwin-arm64",
      profile: "dev-cli",
      toolchainIdentity: "go1.26.6",
      buildVariables: { commit: "abc123" },
    };

    await storeDevRuntime({
      ...identity,
      sourceBinary,
      binaryName: "patchbay",
    });
    const cached = await stageCachedDevRuntime({
      ...identity,
      destinationBinary,
    });

    expect(cached).not.toBeNull();
    expect(await readFile(destinationBinary, "utf8")).toBe("fixture Go CLI");
    expect(await readFile(`${destinationBinary}.sha256`, "utf8")).toMatch(
      new RegExp("^[a-f0-9]{64}\\s{2}patchbay\\n$", "u"),
    );
    const manifest = JSON.parse(
      await readFile(`${destinationBinary}.dev-manifest.json`, "utf8"),
    );
    expect(manifest.target).toBe("darwin-arm64");
    expect(manifest.toolchainIdentity).toBe("go1.26.6");
  });

  it("rejects a corrupted artifact and a mismatched toolchain", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "built", "server");
    await mkdir(join(root, "built"), { recursive: true });
    await writeFile(sourceBinary, "fixture Go server");
    const identity = {
      cacheRoot,
      sourceFingerprint: "source-a",
      target: "darwin-arm64",
      profile: "dev-server",
      toolchainIdentity: "go1.26.6",
      buildVariables: {},
    };
    const stored = await storeDevRuntime({
      ...identity,
      sourceBinary,
      binaryName: "server",
    });

    expect(
      await findCachedDevRuntime({
        ...identity,
        toolchainIdentity: "go1.27.0",
      }),
    ).toBeNull();
    await writeFile(stored.binaryPath, "tampered");
    expect(await findCachedDevRuntime(identity)).toBeNull();
  });
});
