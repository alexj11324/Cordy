import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  defaultDevCliCacheDir,
  devCliCacheKey,
  findCachedDevCli,
  fingerprintRustFiles,
  inspectDevRuntimeCache,
  pruneDevRuntimeCache,
  rustBuildEnvironmentFingerprint,
  rustToolchainIdentity,
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

  it("fingerprints build-affecting Rust environment without including unrelated variables", () => {
    const target = "aarch64-apple-darwin";
    const baseEnv = {
      HOME: "/Users/dev",
      RUSTFLAGS: "-C target-cpu=apple-m1",
    };
    const base = rustBuildEnvironmentFingerprint(baseEnv, target);

    expect(
      rustBuildEnvironmentFingerprint(
        { ...baseEnv, HOME: "/different" },
        target,
      ),
    ).toBe(base);
    for (const changedEnv of [
      { RUSTFLAGS: "-C target-cpu=apple-m2" },
      { CARGO_ENCODED_RUSTFLAGS: "-C\u001ftarget-feature=+aes" },
      { CC: "/opt/llvm/bin/clang" },
      { CC_aarch64_apple_darwin: "/opt/homebrew/bin/clang" },
      { CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER: "zig cc" },
      { CARGO_PROFILE_DEV_LTO: "thin" },
      { CARGO_BUILD_RUSTFLAGS: "-C target-feature=+aes" },
      { CARGO_BUILD_RUSTDOCFLAGS: "--cfg docsrs" },
      { CARGO_BUILD_RUSTC: "/opt/rust/bin/rustc" },
      { CARGO_BUILD_RUSTC_WRAPPER: "/opt/bin/rustc-wrapper" },
      {
        CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER: "/opt/bin/rustc-workspace-wrapper",
      },
    ]) {
      expect(
        rustBuildEnvironmentFingerprint({ ...baseEnv, ...changedEnv }, target),
      ).not.toBe(base);
    }
  });

  it("keys the cache with the compiler resolved beside the selected Cargo", () => {
    const calls = [];
    const identity = rustToolchainIdentity(
      { PATH: "/usr/bin" },
      "/home/dev/.cargo/bin/cargo",
      {
        platform: "linux",
        execFile(command, args, options) {
          calls.push({ command, args, path: options.env.PATH });
          return command.endsWith("cargo")
            ? "cargo 1.91.0\nhost: x86_64-unknown-linux-gnu\n"
            : "rustc 1.91.0\nhost: x86_64-unknown-linux-gnu\n";
        },
      },
    );

    expect(calls).toEqual([
      {
        command: "/home/dev/.cargo/bin/cargo",
        args: ["-vV"],
        path: "/home/dev/.cargo/bin:/usr/bin",
      },
      {
        command: "rustc",
        args: ["-vV"],
        path: "/home/dev/.cargo/bin:/usr/bin",
      },
    ]);
    expect(identity).toContain("cargo 1.91.0");
    expect(identity).toContain("rustc 1.91.0");
  });

  it("does not invent a compiler identity when Cargo is unavailable", () => {
    expect(rustToolchainIdentity({ PATH: "/usr/bin" }, null)).toBeNull();
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

  it("keeps rustc-less staging within one complete runtime identity", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "patchbay-built");
    await writeFile(sourceBinary, "fixture CLI");
    const common = {
      cacheRoot,
      sourceFingerprint: "source-a",
      rustTarget: "aarch64-apple-darwin",
      buildVariables: { version: "dev-source-a" },
    };
    for (const profile of ["dev", "dev-server", "dev-migrate"]) {
      await storeDevCli({
        ...common,
        sourceBinary,
        binaryName: `patchbay-${profile}`,
        profile,
        toolchainIdentity: "rustc one",
      });
    }
    await storeDevCli({
      ...common,
      sourceBinary,
      binaryName: "patchbay-dev-server-newer",
      profile: "dev-server",
      toolchainIdentity: "rustc two",
    });

    const report = await inspectDevRuntimeCache({ cacheRoot });
    const identityKey = report.completeFingerprints.find(
      (entry) => entry.toolchainIdentity === "rustc one",
    )?.identityKey;
    expect(identityKey).toBeTruthy();
    const selected = await findCachedDevCli({
      ...common,
      profile: "dev-server",
      toolchainIdentity: null,
      cacheIdentityKey: identityKey,
    });
    expect(selected?.manifest.toolchainIdentity).toBe("rustc one");
  });

  it("keeps at least ten complete runtime fingerprints while pruning older entries", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "runtime");
    await writeFile(sourceBinary, "fixture runtime");
    for (let index = 0; index < 11; index += 1) {
      for (const profile of ["dev", "dev-server", "dev-migrate"]) {
        await storeDevCli({
          cacheRoot,
          sourceBinary,
          binaryName: `runtime-${profile}`,
          sourceFingerprint: `source-${index}`,
          rustTarget: "aarch64-apple-darwin",
          profile,
          toolchainIdentity: "rustc fixture",
          buildVariables: { index },
        });
      }
    }

    const result = await pruneDevRuntimeCache({
      cacheRoot,
      maxBytes: 0,
      maxAgeMs: 0,
      minFingerprints: 10,
      nowMs: Date.now() + 1_000,
    });
    const after = await inspectDevRuntimeCache({ cacheRoot });

    expect(result.protectedFingerprintCount).toBe(10);
    expect(after.completeFingerprintCount).toBe(10);
    expect(after.entryCount).toBe(30);
  });

  it("does not protect toolchain variants as one runtime fingerprint", async () => {
    const root = await createSandbox();
    const cacheRoot = join(root, "cache");
    const sourceBinary = join(root, "runtime");
    await writeFile(sourceBinary, "fixture runtime");
    for (const toolchainIdentity of ["rustc one", "rustc two"]) {
      for (const profile of ["dev", "dev-server", "dev-migrate"]) {
        await storeDevCli({
          cacheRoot,
          sourceBinary,
          binaryName: `runtime-${profile}`,
          sourceFingerprint: "same-source",
          rustTarget: "aarch64-apple-darwin",
          profile,
          toolchainIdentity,
          buildVariables: {},
        });
      }
    }

    const before = await inspectDevRuntimeCache({ cacheRoot });
    expect(before.completeFingerprintCount).toBe(2);

    const result = await pruneDevRuntimeCache({
      cacheRoot,
      maxBytes: 0,
      maxAgeMs: 0,
      minFingerprints: 1,
      nowMs: Date.now() + 1_000,
    });
    const after = await inspectDevRuntimeCache({ cacheRoot });

    expect(result.protectedFingerprintCount).toBe(1);
    expect(after.entryCount).toBe(3);
    expect(after.completeFingerprintCount).toBe(1);
  });
});
