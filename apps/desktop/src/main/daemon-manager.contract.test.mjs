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
    expect(sync).toContain("const credentialsChanged =");
    expect(sync).toContain("await restartDaemon();");
    expect(sync).not.toContain("void restartDaemon();");
  });
});
