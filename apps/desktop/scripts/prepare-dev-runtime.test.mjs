import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  devRuntimeBuildArguments,
  devRuntimeComponents,
} from "./prepare-dev-runtime.mjs";

describe("complete development runtime artifacts", () => {
  it("stages CLI, backend and migration binaries for one source fingerprint", () => {
    const components = devRuntimeComponents({
      repoRoot: "/repo",
      platform: "darwin",
      arch: "arm64",
      cargoTargetDir: "/repo/server-rs/target",
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
      join("/repo", ".patchbay-dev", "bin", "patchbay-server"),
    );
  });

  it("builds all runtime packages in one incremental Cargo invocation", () => {
    const components = devRuntimeComponents({
      repoRoot: "/repo",
      platform: "linux",
      arch: "x64",
      cargoTargetDir: "/repo/server-rs/target",
    });
    expect(
      devRuntimeBuildArguments("x86_64-unknown-linux-musl", components),
    ).toEqual([
      "build",
      "--locked",
      "--target",
      "x86_64-unknown-linux-musl",
      "-p",
      "patchbay-cli",
      "-p",
      "patchbay-server",
      "-p",
      "patchbay-migrate",
      "--bins",
    ]);
  });
});
