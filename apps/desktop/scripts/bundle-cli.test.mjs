import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  binaryNameForPlatform,
  buildDateForProfile,
  buildProfileFromArgs,
  cargoBuildArguments,
  cargoProfileDirectory,
  cargoTargetDirectory,
  devRustTargetFor,
  devBuildVariables,
  enforceCliAvailability,
  normalizeRuntimeArch,
  normalizeRuntimePlatform,
  rustTargetFor,
  rustBuildEnvironment,
} from "./bundle-cli.mjs";

describe("bundle-cli Rust target selection", () => {
  it.each([
    ["darwin", "x64", "x86_64-apple-darwin"],
    ["darwin", "arm64", "aarch64-apple-darwin"],
    ["linux", "x64", "x86_64-unknown-linux-musl"],
    ["linux", "arm64", "aarch64-unknown-linux-musl"],
    ["win32", "x64", "x86_64-pc-windows-msvc"],
    ["win32", "arm64", "aarch64-pc-windows-msvc"],
  ])("maps %s/%s to %s", (platform, arch, target) => {
    expect(normalizeRuntimePlatform(platform)).toBe(platform);
    expect(normalizeRuntimeArch(arch)).toBe(arch);
    expect(rustTargetFor(platform, arch)).toBe(target);
  });

  it("uses the Windows executable suffix only for Windows targets", () => {
    expect(binaryNameForPlatform("win32")).toBe("patchbay.exe");
    expect(binaryNameForPlatform("linux")).toBe("patchbay");
    expect(binaryNameForPlatform("darwin")).toBe("patchbay");
  });

  it.each([
    ["darwin", "x64", "x86_64-apple-darwin"],
    ["darwin", "arm64", "aarch64-apple-darwin"],
    ["linux", "x64", "x86_64-unknown-linux-gnu"],
    ["linux", "arm64", "aarch64-unknown-linux-gnu"],
    ["win32", "x64", "x86_64-pc-windows-msvc"],
    ["win32", "arm64", "aarch64-pc-windows-msvc"],
  ])(
    "maps development %s/%s to the native host target %s",
    (platform, arch, target) => {
      expect(devRustTargetFor(platform, arch)).toBe(target);
    },
  );

  it("rejects unsupported target combinations", () => {
    expect(() => normalizeRuntimePlatform("freebsd")).toThrow(
      /unsupported target platform/,
    );
    expect(() => normalizeRuntimeArch("ia32")).toThrow(
      /unsupported target architecture/,
    );
    expect(() => rustTargetFor("linux", "ia32")).toThrow(/no Rust target/);
    expect(() => devRustTargetFor("linux", "ia32")).toThrow(
      /no native Rust target/,
    );
  });

  it("uses Cargo's default, relative, and absolute target directory semantics", () => {
    const serverRs = resolve("repo", "server-rs");
    expect(cargoTargetDirectory({}, serverRs)).toBe(join(serverRs, "target"));
    expect(
      cargoTargetDirectory({ CARGO_TARGET_DIR: "../cargo-cache" }, serverRs),
    ).toBe(resolve(serverRs, "../cargo-cache"));
    expect(
      cargoTargetDirectory(
        { CARGO_TARGET_DIR: "/var/cache/patchbay" },
        serverRs,
      ),
    ).toBe(resolve("/var/cache/patchbay"));
  });

  it("requires an explicit profile and makes development incremental", () => {
    const target = "aarch64-apple-darwin";

    expect(() => buildProfileFromArgs([])).toThrow(/explicit --profile/);
    expect(buildProfileFromArgs(["--profile", "release"])).toBe("release");
    expect(buildProfileFromArgs(["--profile", "dev"])).toBe("dev");
    expect(() => buildProfileFromArgs(["--profile", "fast"])).toThrow(
      /unsupported build profile/,
    );
    expect(cargoBuildArguments("release", target)).toContain("--release");
    expect(cargoBuildArguments("dev", target)).not.toContain("--release");
    expect(cargoProfileDirectory("release")).toBe("release");
    expect(cargoProfileDirectory("dev")).toBe("debug");
  });

  it("fails both complete development and formal release when no build input exists", () => {
    expect(() =>
      enforceCliAvailability("release", false, "cargo is unavailable"),
    ).toThrow(/release CLI build is required.*cargo is unavailable/i);
    expect(() =>
      enforceCliAvailability(
        "dev",
        false,
        "cache miss and cargo is unavailable",
      ),
    ).toThrow(/dev CLI build is required.*cache miss/i);
    expect(enforceCliAvailability("release", true, "unused")).toBe(true);
  });

  it("uses stable commit metadata for repeated development bundles", () => {
    const firstNow = new Date("2026-08-30T10:00:00Z");
    const secondNow = new Date("2026-08-30T11:00:00Z");
    const commitDate = "2026-08-29T18:00:00-04:00";

    expect(buildDateForProfile("dev", commitDate, firstNow)).toBe(commitDate);
    expect(buildDateForProfile("dev", commitDate, secondNow)).toBe(commitDate);
    expect(buildDateForProfile("release", commitDate, firstNow)).toBe(
      "2026-08-30T10:00:00Z",
    );
  });

  it("derives cross-worktree development metadata from Rust source only", () => {
    expect(devBuildVariables("abcdef1234567890")).toEqual({
      version: "dev-abcdef123456",
      commit: "source-abcdef123456",
      date: "source-matched-dev",
    });
    expect(devBuildVariables("abcdef1234567890", "env-fingerprint")).toEqual({
      version: "dev-abcdef123456",
      commit: "source-abcdef123456",
      date: "source-matched-dev",
      environmentFingerprint: "env-fingerprint",
    });
  });

  it("uses sccache when present without overriding an explicit compiler wrapper", () => {
    expect(rustBuildEnvironment({}, true)).toEqual({
      RUSTC_WRAPPER: "sccache",
    });
    expect(rustBuildEnvironment({ RUSTC_WRAPPER: "custom" }, true)).toEqual({
      RUSTC_WRAPPER: "custom",
    });
    expect(
      rustBuildEnvironment({ PATCHBAY_DISABLE_SCCACHE: "1" }, true),
    ).toEqual({ PATCHBAY_DISABLE_SCCACHE: "1" });
  });
});
