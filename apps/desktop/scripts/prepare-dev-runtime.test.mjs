import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  devRuntimeBuildArguments,
  devRuntimeBuildEnvironment,
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
      join("/repo", ".patchbay-dev", "bin", "patchbay"),
    );
    expect(components[0].additionalDestinations).toEqual([
      join("/repo", "apps", "desktop", "resources", "bin", "patchbay"),
    ]);
    expect(components[1].destinationBinary).toBe(
      join("/repo", ".patchbay-dev", "bin", "patchbay-server"),
    );
  });

  it("keeps development Cargo outputs inside each worktree", () => {
    const components = devRuntimeComponents({
      repoRoot: "/repo/worktree-a",
      platform: "linux",
      arch: "x64",
    });

    expect(components[0].sourceBinary).toBe(
      join(
        "/repo/worktree-a",
        "server-rs",
        "target",
        "x86_64-unknown-linux-gnu",
        "debug",
        "patchbay",
      ),
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
      devRuntimeBuildArguments("x86_64-unknown-linux-gnu", components),
    ).toEqual([
      "build",
      "--locked",
      "--target",
      "x86_64-unknown-linux-gnu",
      "-p",
      "patchbay-cli",
      "-p",
      "patchbay-server",
      "-p",
      "patchbay-migrate",
      "--bins",
    ]);
  });

  it("keeps rustup proxies discoverable when Cargo is resolved outside PATH", () => {
    const env = { PATH: "/usr/bin", RUSTC_WRAPPER: "sccache" };
    expect(
      devRuntimeBuildEnvironment(env, "/home/dev/.cargo/bin/cargo"),
    ).toMatchObject({
      PATH: expect.stringMatching(/^\/home\/dev\/\.cargo\/bin:/),
      RUSTC_WRAPPER: "sccache",
    });
  });
});
