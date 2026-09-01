import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const workflow = (name) =>
  readFileSync(resolve(repoRoot, ".github", "workflows", name), "utf8");

describe("CI cache and release artifact contract", () => {
  it.each(["ci.yml", "release.yml", "macos-release.yml"])(
    "%s caches downloads/compiler objects but never a Rust target directory",
    (name) => {
      const contents = workflow(name);
      expect(contents).not.toContain("Swatinem/rust-cache");
      expect(contents).not.toMatch(
        /uses: actions\/cache@[^\n]*\n(?:[^\n]*\n){0,12}[^\n]*path:[^\n]*(?:server-rs\/)?target/,
      );
      expect(contents).toContain(
        "mozilla-actions/sccache-action@fc920bf0ec8de6ee65d409111f7ec508035751ba # v0.0.11",
      );
      expect(contents).toContain("~/.cargo/registry");
      expect(contents).toContain("~/.cargo/git");
      expect(contents).toContain("hashFiles('rust-toolchain.toml')");
    },
  );

  it("does not persist container BuildKit target trees", () => {
    const imageWorkflow = workflow("aspectlylabs-production-images.yml");
    expect(imageWorkflow).not.toContain("buildkit-cache-dance");
    expect(imageWorkflow).not.toContain(".buildkit-cache/");
    expect(imageWorkflow).not.toMatch(
      /uses: actions\/cache@[^\n]*\n(?:[^\n]*\n){0,12}[^\n]*path:[^\n]*(?:server-rs\/)?target/,
    );
  });

  it("builds the Desktop smoke CLI once per target and packages the exact artifact", () => {
    const contents = workflow("desktop-smoke.yml");
    expect(contents).toContain("cli-build:");
    expect(contents).toContain("desktop-smoke-cli-${{ matrix.target }}-${{ matrix.arch }}");
    expect(contents).toContain("PATCHBAY_PREBUILT_CLI_DIR: ${{ runner.temp }}/patchbay-cli-bin");
    expect(contents).toContain("desktop-smoke-cargo-${{ runner.os }}-${{ runner.arch }}-${{ matrix.rust_target }}-");
    expect(contents).toContain("mozilla-actions/sccache-action@fc920bf0ec8de6ee65d409111f7ec508035751ba # v0.0.11");
    expect(contents).not.toMatch(/path:[^\n]*target(?:\/|\\)/u);
  });

  it("builds each release CLI once and feeds the exact artifact to Desktop packaging", () => {
    const contents = workflow("release.yml");
    expect(contents).toContain(
      "name: desktop-cli-bin-${{ matrix.os }}-${{ matrix.arch }}",
    );
    expect(contents).toContain(
      "PATCHBAY_PREBUILT_CLI_DIR: ${{ runner.temp }}/patchbay-cli-bin",
    );
    expect(contents).toContain("write-cli-artifact-manifest.mjs");
    expect(contents).not.toContain(
      "cargo build --release --locked -p patchbay-server -p patchbay-cli",
    );
  });
});
