import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sourcePath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "daemon-manager.ts",
);
const rustConfigPath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../../..",
  "server-rs/crates/patchbay-cli/src/config.rs",
);

describe("daemon manager mutation contracts", () => {
  it("retires the legacy profile with the unlocked cleanup path inside target switching", () => {
    const source = readFileSync(sourcePath, "utf8");
    const handlerStart = source.indexOf(
      'ipcMain.handle("daemon:set-target-api-url"',
    );
    expect(handlerStart).toBeGreaterThan(-1);

    const handler = source.slice(handlerStart);
    expect(handler).toContain(
      "await retirePendingLegacyProfile(clearProfileCredentialsUnlocked)",
    );
    expect(handler).not.toContain(
      "await retirePendingLegacyProfile(clearProfileCredentials);",
    );
  });

  it("awaits credential-triggered daemon restarts before releasing the mutation queue", () => {
    const source = readFileSync(sourcePath, "utf8");
    const syncStart = source.indexOf("async function syncTokenUnlocked");
    const syncEnd = source.indexOf("async function loadPrefs", syncStart);
    expect(syncStart).toBeGreaterThan(-1);
    expect(syncEnd).toBeGreaterThan(syncStart);

    const sync = source.slice(syncStart, syncEnd);
    expect(sync).toContain("return commitDesktopCredentials({");
    expect(sync).toContain("stopDaemon: stopDaemonUnlocked");
    expect(sync).toContain("prepareCredentialChange:");
    expect(sync).toContain("restartDaemon: restartDaemonUnlocked");
    expect(sync).not.toContain("void restartDaemon();");

    const reauthStart = source.indexOf("async function reauthenticateUnlocked");
    const reauthEnd = source.indexOf("async function withGuard", reauthStart);
    expect(reauthStart).toBeGreaterThan(-1);
    expect(reauthEnd).toBeGreaterThan(reauthStart);
    const reauth = source.slice(reauthStart, reauthEnd);
    expect(reauth).toContain("forceRefresh: true");
    expect(reauth).not.toContain("clearProfileCredentialsUnlocked");

    const lifecycleStart = source.indexOf("async function startDaemonUnlocked");
    const lifecycleEnd = source.indexOf("async function pollOnce", lifecycleStart);
    expect(lifecycleStart).toBeGreaterThan(-1);
    expect(lifecycleEnd).toBeGreaterThan(lifecycleStart);
    const lifecycle = source.slice(lifecycleStart, lifecycleEnd);
    expect(lifecycle).toContain(
      "return serializeProfileMutation(async () => {",
    );
    expect(lifecycle).toContain(
      "return serializeProfileMutation(() => stopDaemonUnlocked())",
    );
    expect(lifecycle).toContain("return restartDaemonUnlocked();");
    expect(lifecycle).toContain("credentials unavailable:");

    const targetStart = source.indexOf(
      'ipcMain.handle("daemon:set-target-api-url"',
    );
    const target = source.slice(targetStart);
    expect(target).toContain(
      'blockCredentialSync("credentials are not synchronized")',
    );

    const probeStart = source.indexOf("async function probeLocalRuntimes");
    const probeEnd = source.indexOf(
      "// Env passed to every CLI child",
      probeStart,
    );
    expect(probeStart).toBeGreaterThan(-1);
    expect(probeEnd).toBeGreaterThan(probeStart);
    expect(source.slice(probeStart, probeEnd)).toContain(
      'if (credentialSyncError) return { probeResult: "error" };',
    );

    const logStart = source.indexOf("function sendLines");
    const setupStart = source.indexOf("export function setupDaemonManager");
    expect(logStart).toBeGreaterThan(-1);
    expect(setupStart).toBeGreaterThan(logStart);
    const logRuntime = source.slice(logStart, setupStart);
    expect(logRuntime).toContain("if (credentialSyncError) return;");
    expect(logRuntime).toContain("credentialSyncGeneration");
    expect(logRuntime).toContain("stopLogTail();");
    expect(source).toContain('ipcMain.on("daemon:start-log-stream", () => {');
    expect(source).toContain('if (credentialSyncError) return;');
    expect(source).toContain('ipcMain.handle("daemon:open-log-file", () =>');
    expect(source).toContain("credentials unavailable:");
    expect(source).toContain('webContents.send("daemon:log-reset")');

    const mintStart = source.indexOf("async function mintPat");
    const mintEnd = source.indexOf("/**\n * Ensure the active profile", mintStart);
    expect(mintStart).toBeGreaterThan(-1);
    expect(mintEnd).toBeGreaterThan(mintStart);
    expect(source.slice(mintStart, mintEnd)).not.toContain("res.text");
    expect(source.slice(mintStart, mintEnd)).toContain("HTTP ${res.status}");

    const clearStart = source.indexOf("async function clearToken");
    const clearEnd = source.indexOf("async function clearProfileCredentials", clearStart);
    expect(clearStart).toBeGreaterThan(-1);
    expect(clearEnd).toBeGreaterThan(clearStart);
    const clear = source.slice(clearStart, clearEnd);
    expect(clear).toContain("stopDaemonUnlocked");
    expect(clear.indexOf("stopDaemonUnlocked")).toBeLessThan(
      clear.indexOf("clearProfileCredentialsUnlocked"),
    );

    expect(source).toContain("const DESKTOP_BLOCKED_ENV_KEYS = [");
    expect(source).toContain('"PATCHBAY_DEBUG"');
    expect(source).toContain('"PATCHBAY_TASK_CONFIG_ROOT"');
    expect(source).toContain("for (const key of DESKTOP_BLOCKED_ENV_KEYS)");
    expect(source).toContain("env: desktopSpawnEnv()");
  });

  it("keeps the Desktop child env blocklist in lockstep with the Rust capture set", () => {
    const source = readFileSync(sourcePath, "utf8");
    const rustSource = readFileSync(rustConfigPath, "utf8");
    const desktopBlock = source.match(
      /const DESKTOP_BLOCKED_ENV_KEYS = \[(.*?)\] as const/s,
    )?.[1];
    const rustBlock = rustSource.match(
      /const CAPTURED_ENV_KEYS: &[\s\S]*?= &\[(.*?)\];/s,
    )?.[1];
    expect(desktopBlock).toBeDefined();
    expect(rustBlock).toBeDefined();

    const quotedKeys = (block) =>
      [...block.matchAll(/"([A-Z0-9_]+)"/g)].map((match) => match[1]);
    const taskRoot = rustSource.match(
      /pub const TASK_CONFIG_ROOT_ENV: &str = "([A-Z0-9_]+)";/,
    )?.[1];
    const rustKeys = [
      ...quotedKeys(rustBlock),
      ...(rustBlock.includes("TASK_CONFIG_ROOT_ENV") && taskRoot
        ? [taskRoot]
        : []),
    ];
    expect(new Set(quotedKeys(desktopBlock))).toEqual(new Set(rustKeys));

    const probeStart = source.indexOf("async function probeCliBinary");
    const probeEnd = source.indexOf("/**\n * Returns a usable", probeStart);
    expect(source.slice(probeStart, probeEnd)).toContain(
      "env: desktopSpawnEnv()",
    );
  });
});
