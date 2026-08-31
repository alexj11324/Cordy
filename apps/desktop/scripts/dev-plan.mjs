import { join } from "node:path";

export function planDevCommands(
  argv,
  { nodePath = process.execPath, scriptsDir },
) {
  for (const token of argv) {
    if (token === "--bundle-cli" || token === "--source-cli") {
      throw new Error(
        "[dev:desktop] the Rust-free/source toggle was removed; `pnpm dev` always runs the complete source-matched environment",
      );
    }
  }

  return [
    {
      command: nodePath,
      args: [join(scriptsDir, "dev-environment-doctor.mjs")],
    },
    {
      command: nodePath,
      args: [join(scriptsDir, "brand-dev-electron.mjs")],
    },
    {
      command: "electron-vite",
      args: ["dev", ...argv],
    },
  ];
}
