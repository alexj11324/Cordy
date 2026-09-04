// @vitest-environment node
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  devRuntimeComponents,
  goBuildArguments,
} from "./prepare-dev-runtime.mjs";

describe("complete Go development runtime artifacts", () => {
  it("stages CLI, backend and migration binaries for one source fingerprint", () => {
    const components = devRuntimeComponents({
      repoRoot: "/repo",
      platform: "darwin",
      arch: "arm64",
    });
    expect(components.map(({ id }) => id)).toEqual([
      "cli",
      "backend",
      "migrations",
    ]);
    expect(components[0].destinationBinary).toBe(
      join("/repo", "apps", "desktop", "resources", "bin", "patchbay"),
    );
    expect(components[1].destinationBinary).toBe(
      join("/repo", ".patchbay-dev", "bin", "server"),
    );
    expect(components[2].destinationBinary).toBe(
      join("/repo", ".patchbay-dev", "bin", "migrate"),
    );
  });

  it("uses the target Go environment and version flags for the CLI", () => {
    const [cli] = devRuntimeComponents({
      repoRoot: "/repo",
      platform: "win32",
      arch: "x64",
    });
    expect(goBuildArguments(cli, {
      version: "dev",
      commit: "abc123",
      date: "2026-09-04T00:00:00Z",
    })).toEqual([
      "build",
      "-trimpath",
      "-ldflags",
      "-X main.version=dev -X main.commit=abc123 -X main.date=2026-09-04T00:00:00Z",
      "-o",
      join("/repo", "server", "bin", "windows-amd64", "patchbay.exe"),
      "./cmd/patchbay",
    ]);
  });

  it("does not pass CLI-only date flags to the migration binary", () => {
    const [, , migrations] = devRuntimeComponents({
      repoRoot: "/repo",
      platform: "linux",
      arch: "x64",
    });
    expect(goBuildArguments(migrations, {
      version: "dev",
      commit: "abc123",
      date: "2026-09-04T00:00:00Z",
    })).toEqual([
      "build",
      "-trimpath",
      "-o",
      join("/repo", "server", "bin", "linux-amd64", "migrate"),
      "./cmd/migrate",
    ]);
  });
});
