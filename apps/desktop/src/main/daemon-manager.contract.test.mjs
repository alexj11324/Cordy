import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sourcePath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "daemon-manager.ts",
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
    expect(logRuntime).toContain("stopLogTail();");
    expect(source).toContain('ipcMain.on("daemon:start-log-stream", () => {');
    expect(source).toContain('if (credentialSyncError) return;');
    expect(source).toContain('ipcMain.handle("daemon:open-log-file", async () => {');
    expect(source).toContain("credentials unavailable:");
  });
});
