import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  binaryNameForPlatform,
  cargoTargetDirectory,
  normalizeRuntimeArch,
  normalizeRuntimePlatform,
  rustTargetFor,
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

  it("rejects unsupported target combinations", () => {
    expect(() => normalizeRuntimePlatform("freebsd")).toThrow(
      /unsupported target platform/,
    );
    expect(() => normalizeRuntimeArch("ia32")).toThrow(
      /unsupported target architecture/,
    );
    expect(() => rustTargetFor("linux", "ia32")).toThrow(/no Rust target/);
  });

  it("uses Cargo's default, relative, and absolute target directory semantics", () => {
    const serverRs = resolve("repo", "server-rs");
    expect(cargoTargetDirectory({}, serverRs)).toBe(join(serverRs, "target"));
    expect(
      cargoTargetDirectory({ CARGO_TARGET_DIR: "../cargo-cache" }, serverRs),
    ).toBe(resolve(serverRs, "../cargo-cache"));
    expect(
      cargoTargetDirectory({ CARGO_TARGET_DIR: "/var/cache/patchbay" }, serverRs),
    ).toBe(resolve("/var/cache/patchbay"));
  });
});
