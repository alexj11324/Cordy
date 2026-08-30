import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { planDevCommands } from "./dev-plan.mjs";

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const desktopPackage = JSON.parse(
  readFileSync(join(scriptsDir, "..", "package.json"), "utf8"),
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
      planDevCommands(["--bundle-cli", "--mode", "staging"], {
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

  it("keeps ordinary builds Rust-free and provides the source-matched dev entry", () => {
    expect(desktopPackage.scripts.build).toBe("electron-vite build");
    expect(desktopPackage.scripts["dev:rust"]).toBe(
      "node scripts/dev.mjs --bundle-cli",
    );
  });
});
