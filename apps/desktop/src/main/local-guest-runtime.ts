import { app, BrowserWindow, ipcMain } from "electron";
import { execFile } from "node:child_process";
import { constants as fsConstants } from "node:fs";
import { access } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";
import type { LocalRuntimeProbe } from "../shared/daemon-types";
import { parseLocalRuntimeProbe } from "../shared/local-guest";

const execFileAsync = promisify(execFile);
const LOCAL_RUNTIME_PROBE_TIMEOUT_MS = 15_000;
const CLI_VERSION_PROBE_TIMEOUT_MS = 5_000;

export function bundledCliPath(
  appPath: string,
  platform: NodeJS.Platform = process.platform,
): string {
  const binaryName = platform === "win32" ? "patchbay.exe" : "patchbay";
  return join(appPath, "resources", "bin", binaryName).replace(
    "app.asar",
    "app.asar.unpacked",
  );
}

function localGuestChildEnvironment(): NodeJS.ProcessEnv {
  // Do not inherit PATCHBAY_* settings: they can point the CLI at a cloud
  // profile, task context, or remote server. The local probe only needs the
  // host's command lookup environment and locale.
  const allowedKeys = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
  ];
  const environment: NodeJS.ProcessEnv = {};
  for (const key of allowedKeys) {
    const value = process.env[key];
    if (value !== undefined) environment[key] = value;
  }
  return environment;
}

async function verifyBundledCli(binaryPath: string): Promise<boolean> {
  try {
    await access(binaryPath, fsConstants.X_OK);
    const { stdout } = await execFileAsync(
      binaryPath,
      ["version", "--output", "json"],
      {
        timeout: CLI_VERSION_PROBE_TIMEOUT_MS,
        env: localGuestChildEnvironment(),
        maxBuffer: 64 * 1024,
      },
    );
    const parsed = JSON.parse(stdout) as { version?: unknown };
    return typeof parsed.version === "string" && parsed.version.length > 0;
  } catch {
    return false;
  }
}

export async function probeBundledLocalRuntimes(
  appPath = app.getAppPath(),
): Promise<LocalRuntimeProbe> {
  const binaryPath = bundledCliPath(appPath);
  if (!(await verifyBundledCli(binaryPath))) {
    return { probeResult: "error" };
  }

  try {
    const { stdout } = await execFileAsync(
      binaryPath,
      ["daemon", "probe-runtimes", "--local"],
      {
        timeout: LOCAL_RUNTIME_PROBE_TIMEOUT_MS,
        env: localGuestChildEnvironment(),
        maxBuffer: 128 * 1024,
      },
    );
    return parseLocalRuntimeProbe(JSON.parse(stdout) as unknown);
  } catch {
    return { probeResult: "error" };
  }
}

function isMainWindowSender(
  event: Electron.IpcMainInvokeEvent,
  getMainWindow: () => BrowserWindow | null,
): boolean {
  const senderWindow = BrowserWindow.fromWebContents(event.sender);
  const mainWindow = getMainWindow();
  return Boolean(
    senderWindow &&
      mainWindow &&
      senderWindow === mainWindow &&
      !senderWindow.isDestroyed(),
  );
}

export function setupLocalGuestRuntime(
  getMainWindow: () => BrowserWindow | null,
): void {
  ipcMain.handle(
    "guest-runtime:probe",
    async (event): Promise<LocalRuntimeProbe> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { probeResult: "error" };
      }
      return probeBundledLocalRuntimes();
    },
  );
}
