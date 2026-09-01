import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { parseDevRuntimeArgs } from "../../../scripts/dev-runtime-profile.mjs";
import { planDevCommands } from "./dev-plan.mjs";

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptsDir, "..", "..", "..");
const desktopPackage = JSON.parse(
  readFileSync(join(scriptsDir, "..", "package.json"), "utf8"),
);
const rootPackage = JSON.parse(
  readFileSync(join(repoRoot, "package.json"), "utf8"),
);
const devLauncher = readFileSync(join(scriptsDir, "dev.mjs"), "utf8");
const ciWorkflow = readFileSync(
  join(repoRoot, ".github", "workflows", "ci.yml"),
  "utf8",
);

describe("Desktop development build plan", () => {
  it("requires the capability doctor before Electron", () => {
    expect(
      planDevCommands([], { nodePath: "/usr/bin/node", scriptsDir }),
    ).toEqual([
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "dev-environment-doctor.mjs")],
      },
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "brand-dev-electron.mjs")],
      },
      { command: "electron-vite", args: ["dev"] },
    ]);
  });

  it("passes Electron-specific arguments without changing the complete stack", () => {
    expect(
      planDevCommands(["--inspect"], {
        nodePath: "/usr/bin/node",
        scriptsDir,
      }),
    ).toEqual([
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "dev-environment-doctor.mjs")],
      },
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "brand-dev-electron.mjs")],
      },
      {
        command: "electron-vite",
        args: ["dev", "--inspect"],
      },
    ]);
  });

  it("never forwards the hosted launcher flag to electron-vite", () => {
    const { electronArgs } = parseDevRuntimeArgs(["--hosted", "--inspect"]);
    expect(
      planDevCommands(electronArgs, {
        nodePath: "/usr/bin/node",
        scriptsDir,
      }),
    ).toEqual([
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "dev-environment-doctor.mjs")],
      },
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "brand-dev-electron.mjs")],
      },
      { command: "electron-vite", args: ["dev", "--inspect"] },
    ]);
  });

  it("applies the hosted storage identity after worktree isolation", () => {
    expect(devLauncher).toContain("applyWorktreeDevEnv(process.env");
    expect(devLauncher).toContain("applyDevRuntimeAppIdentity(process.env)");
    expect(devLauncher).toContain("withoutDevClerkEnvironment(process.env)");
    expect(devLauncher).toContain("const isDoctor");
    expect(devLauncher).toContain("env: isDoctor");
    expect(devLauncher).toContain("clearDevClerkEnvironment()");
    expect(devLauncher.lastIndexOf("applyDevRuntimeAppIdentity")).toBeGreaterThan(
      devLauncher.indexOf("applyWorktreeDevEnv(process.env"),
    );
  });

  it.each(["--bundle-cli", "--source-cli"])(
    "rejects the removed Rust-free/source toggle %s",
    (flag) => {
      expect(() =>
        planDevCommands([flag], {
          nodePath: "/usr/bin/node",
          scriptsDir,
        }),
      ).toThrow(/toggle was removed.*always runs the complete/i);
    },
  );

  it("exposes one complete development entry and no UI-only alternative", () => {
    expect(desktopPackage.scripts.build).toBe("electron-vite build");
    expect(desktopPackage.scripts["bundle-cli"]).toBeUndefined();
    expect(desktopPackage.scripts["bundle-cli:release"]).toBeUndefined();
    expect(desktopPackage.scripts.dev).toBe(
      "node ../../scripts/dev-launcher.mjs",
    );
    expect(desktopPackage.scripts["dev:rust"]).toBeUndefined();
    expect(desktopPackage.scripts["dev:web"]).toBeUndefined();
    expect(rootPackage.scripts["dev:desktop"]).toBe(
      "node scripts/dev-launcher.mjs",
    );
    expect(rootPackage.scripts.dev).toBe("node scripts/dev-launcher.mjs");
    expect(rootPackage.engines.node).toBe(">=22 <23");
    expect(rootPackage.packageManager).toBe("pnpm@10.28.2");
    expect(rootPackage.devEngines.runtime).toMatchObject({
      name: "node",
      version: "^22.0.0",
      onFail: "download",
    });
    expect(rootPackage.scripts["dev:desktop:rust"]).toBeUndefined();
    expect(rootPackage.scripts["dev:desktop:web"]).toBeUndefined();
    expect(rootPackage.scripts.build).toBe(
      "turbo build --filter=!@patchbay/mobile",
    );
    expect(devLauncher).not.toContain('"bundle-cli.mjs"');
    expect(ciWorkflow).not.toContain("Prepare Rust CLI bundle target");
  });
});
