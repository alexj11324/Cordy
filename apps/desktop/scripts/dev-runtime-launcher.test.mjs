// @vitest-environment node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const desktopDev = readFileSync(
  resolve(import.meta.dirname, "dev.mjs"),
  "utf8",
);
const rootDev = readFileSync(resolve(repoRoot, "scripts", "dev.sh"), "utf8");
const gitignore = readFileSync(resolve(repoRoot, ".gitignore"), "utf8");
const bundleCli = readFileSync(
  resolve(import.meta.dirname, "bundle-cli.mjs"),
  "utf8",
);
const daemonManager = readFileSync(
  resolve(repoRoot, "apps", "desktop", "src", "main", "daemon-manager.ts"),
  "utf8",
);

describe("Go development runtime launcher contract", () => {
  it("prepares source-matched Go artifacts before opening Electron", () => {
    expect(desktopDev).toContain("prepare-dev-runtime.mjs");
    expect(desktopDev).not.toContain('"bundle-cli.mjs"');
    expect(desktopDev).toContain("PATCHBAY_REQUIRE_SOURCE_CLI");
    expect(rootDev).toContain("prepare-dev-runtime.mjs");
    expect(rootDev).toContain(".patchbay-dev/bin/server");
    expect(rootDev).toContain(".patchbay-dev/bin/migrate");
    expect(gitignore).toContain(".patchbay-dev/");
    expect(rootDev).not.toContain("go run ./cmd/server");
    expect(rootDev).not.toContain("go run ./cmd/migrate");
  });

  it("never turns a missing Go build into a release or PATH fallback", () => {
    expect(bundleCli).not.toContain("auto-installing the latest release");
    expect(bundleCli).not.toContain("go not found in PATH");
    expect(daemonManager).toContain("PATCHBAY_REQUIRE_SOURCE_CLI");
    expect(daemonManager).toContain(
      "source CLI required but the bundled Go CLI is missing or invalid",
    );
  });
});
