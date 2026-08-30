import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

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
  it("starts Electron without compiling the Rust CLI by default", () => {
    expect(
      planDevCommands([], { nodePath: "/usr/bin/node", scriptsDir }),
    ).toEqual([
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "brand-dev-electron.mjs")],
      },
      { command: "electron-vite", args: ["dev"] },
    ]);
  });

  it("uses an explicit incremental Rust development build when requested", () => {
    expect(
      planDevCommands(["--source-cli", "--mode", "staging"], {
        nodePath: "/usr/bin/node",
        scriptsDir,
      }),
    ).toEqual([
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "bundle-cli.mjs"), "--profile", "dev"],
      },
      {
        command: "/usr/bin/node",
        args: [join(scriptsDir, "brand-dev-electron.mjs")],
      },
      {
        command: "electron-vite",
        args: ["dev", "--mode", "staging"],
      },
    ]);
  });

  it("rejects the removed ambiguous bundle flag", () => {
    expect(() =>
      planDevCommands(["--bundle-cli"], {
        nodePath: "/usr/bin/node",
        scriptsDir,
      }),
    ).toThrow(/removed.*dev:desktop:rust/i);
  });

  it("keeps ordinary builds Rust-free and provides the source-matched dev entry", () => {
    expect(desktopPackage.scripts.build).toBe("electron-vite build");
    expect(desktopPackage.scripts["bundle-cli"]).toBeUndefined();
    expect(desktopPackage.scripts["bundle-cli:release"]).toBeUndefined();
    expect(desktopPackage.scripts["dev:rust"]).toBe(
      "node scripts/dev.mjs --source-cli",
    );
    expect(rootPackage.scripts["dev:desktop"]).toBe(
      "turbo dev --filter=@patchbay/desktop",
    );
    expect(rootPackage.scripts.build).toBe(
      "turbo build --filter=!@patchbay/mobile",
    );
    expect(devLauncher).not.toContain('"bundle-cli.mjs"');
    expect(ciWorkflow).not.toContain("Prepare Rust CLI bundle target");
  });
});
