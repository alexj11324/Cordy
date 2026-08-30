import { join } from "node:path";

export function planDevCommands(
  argv,
  { nodePath = process.execPath, scriptsDir },
) {
  const electronViteArgs = [];
  let bundleCli = false;

  for (const token of argv) {
    if (token === "--bundle-cli") {
      throw new Error(
        "[dev:desktop] --bundle-cli was removed; use pnpm dev:desktop:rust for source-matched Rust development",
      );
    }
    if (token === "--source-cli") {
      bundleCli = true;
    } else {
      electronViteArgs.push(token);
    }
  }

  const commands = [];
  if (bundleCli) {
    commands.push({
      command: nodePath,
      args: [join(scriptsDir, "bundle-cli.mjs"), "--profile", "dev"],
    });
  } else {
    commands.push({
      command: nodePath,
      args: [join(scriptsDir, "clear-dev-cli.mjs")],
    });
  }

  commands.push(
    {
      command: nodePath,
      args: [join(scriptsDir, "brand-dev-electron.mjs")],
    },
    {
      command: "electron-vite",
      args: ["dev", ...electronViteArgs],
    },
  );

  return commands;
}
