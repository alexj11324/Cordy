// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserWindow, WebContents } from "electron";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

type Handler = (...args: unknown[]) => void;
type IpcHandler = (...args: unknown[]) => unknown;

const ctx = vi.hoisted(() => ({
  handlers: new Map<string, Handler[]>(),
  ipcHandlers: new Map<string, IpcHandler>(),
  ipcHandle: vi.fn(),
  checkForUpdates: vi.fn(async () => ({
    updateInfo: { version: "0.3.18" },
    isUpdateAvailable: false,
  })),
  downloadUpdate: vi.fn(),
  quitAndInstall: vi.fn(),
  getVersion: vi.fn(() => "0.3.17"),
  userDataPath: "",
  preferredLanguages: ["en-US"] as string[],
  showMessageBox: vi.fn(async () => ({ response: 0, checkboxChecked: false })),
}));

vi.mock("electron-updater", () => {
  const autoUpdater = {
    autoDownload: false,
    autoInstallOnAppQuit: false,
    channel: undefined as string | undefined,
    allowDowngrade: false,
    on: vi.fn((event: string, handler: Handler) => {
      const handlers = ctx.handlers.get(event) ?? [];
      handlers.push(handler);
      ctx.handlers.set(event, handlers);
      return autoUpdater;
    }),
    removeListener: vi.fn((event: string, handler: Handler) => {
      const handlers = (ctx.handlers.get(event) ?? []).filter(
        (candidate) => candidate !== handler,
      );
      ctx.handlers.set(event, handlers);
      return autoUpdater;
    }),
    checkForUpdates: ctx.checkForUpdates,
    downloadUpdate: ctx.downloadUpdate,
    quitAndInstall: ctx.quitAndInstall,
  };
  return { autoUpdater };
});

vi.mock("electron", () => ({
  app: {
    getVersion: ctx.getVersion,
    getPath: vi.fn(() => ctx.userDataPath),
    getPreferredSystemLanguages: () => ctx.preferredLanguages,
  },
  BrowserWindow: class BrowserWindow {},
  dialog: {
    showMessageBox: ctx.showMessageBox,
  },
  ipcMain: {
    handle: ctx.ipcHandle,
    removeHandler: (channel: string) => {
      ctx.ipcHandlers.delete(channel);
    },
  },
}));

import {
  configureMacX64UpdateChannel,
  resetUpdaterTransientStateForTests,
  runMenuUpdateCheck,
  setupAutoUpdater,
} from "./updater";
import { updaterPreferencesPath } from "./updater-preferences";

describe("macOS x64 update channel", () => {
  it("does not touch established architecture paths", () => {
    for (const [platform, arch] of [
      ["darwin", "arm64"],
      ["win32", "x64"],
      ["win32", "arm64"],
      ["linux", "arm64"],
    ] as const) {
      const updater = { channel: null, allowDowngrade: true };

      configureMacX64UpdateChannel(updater, platform, arch);

      expect(updater).toEqual({ channel: null, allowDowngrade: true });
    }
  });

  it("does not enable downgrades when selecting an architecture feed", () => {
    const updater = { channel: null, allowDowngrade: true };

    configureMacX64UpdateChannel(updater, "darwin", "x64");

    expect(updater).toEqual({
      channel: "latest-x64",
      allowDowngrade: false,
    });
  });
});

function emitUpdater(event: string, ...args: unknown[]) {
  for (const handler of ctx.handlers.get(event) ?? []) {
    handler(...args);
  }
}

async function invokeIpc(channel: string, ...args: unknown[]) {
  const handler = ctx.ipcHandlers.get(channel);
  if (!handler) throw new Error(`Missing IPC handler: ${channel}`);
  return handler({}, ...args);
}

function makeWindow() {
  const send = vi.fn();
  return {
    win: {
      isDestroyed: () => false,
      webContents: {
        isDestroyed: () => false,
        send,
      },
    } as unknown as BrowserWindow,
    send,
  };
}

function makeDestroyedWindow() {
  return {
    isDestroyed: () => true,
    get webContents(): WebContents {
      throw new TypeError("Object has been destroyed");
    },
  } as unknown as BrowserWindow;
}

function makeWindowWithDestroyedWebContents() {
  const send = vi.fn(() => {
    throw new TypeError("Object has been destroyed");
  });
  return {
    win: {
      isDestroyed: () => false,
      webContents: {
        isDestroyed: () => true,
        send,
      },
    } as unknown as BrowserWindow,
    send,
  };
}

function makeWindowWithThrowingSend(error: Error) {
  const send = vi.fn(() => {
    throw error;
  });
  return {
    win: {
      isDestroyed: () => false,
      webContents: {
        isDestroyed: () => false,
        send,
      },
    } as unknown as BrowserWindow,
    send,
  };
}

describe("setupAutoUpdater", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    ctx.userDataPath = mkdtempSync(join(tmpdir(), "patchbay-updater-test-"));
    ctx.handlers.clear();
    ctx.ipcHandlers.clear();
    ctx.ipcHandle.mockClear();
    ctx.ipcHandle.mockImplementation((channel: string, handler: IpcHandler) => {
      ctx.ipcHandlers.set(channel, handler);
    });
    ctx.checkForUpdates.mockClear();
    ctx.downloadUpdate.mockClear();
    ctx.quitAndInstall.mockClear();
    ctx.getVersion.mockClear();
    ctx.preferredLanguages = ["en-US"];
    ctx.showMessageBox.mockClear();
    ctx.showMessageBox.mockResolvedValue({
      response: 0,
      checkboxChecked: false,
    });
    resetUpdaterTransientStateForTests();
  });

  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    rmSync(ctx.userDataPath, { recursive: true, force: true });
  });

  it("enables automatic background updates by default", async () => {
    setupAutoUpdater(() => null);

    await expect(invokeIpc("updater:get-preferences")).resolves.toEqual({
      automaticUpdates: true,
    });

    await vi.advanceTimersByTimeAsync(5_000);
    expect(ctx.checkForUpdates).toHaveBeenCalledTimes(1);
  });

  it("skips startup and periodic checks when automatic updates are disabled", async () => {
    writeFileSync(
      updaterPreferencesPath(ctx.userDataPath),
      JSON.stringify({ automaticUpdates: false }),
    );
    setupAutoUpdater(() => null);

    // Let the async preference load settle before advancing timers; otherwise
    // the in-flight readFile can resolve after afterEach() removes the temp
    // dir, default back to enabled=true, and fire a background check into the
    // next test's freshly-cleared mock (flake on slow CI).
    await invokeIpc("updater:get-preferences");

    await vi.advanceTimersByTimeAsync(60 * 60 * 1000 + 5_000);

    expect(ctx.checkForUpdates).not.toHaveBeenCalled();
  });

  it("persists the automatic update preference and stops future background checks", async () => {
    setupAutoUpdater(() => null);

    await expect(
      invokeIpc("updater:set-automatic-updates", false),
    ).resolves.toEqual({ automaticUpdates: false });
    expect(
      JSON.parse(
        readFileSync(updaterPreferencesPath(ctx.userDataPath), "utf-8"),
      ),
    ).toEqual({ automaticUpdates: false });

    await vi.advanceTimersByTimeAsync(60 * 60 * 1000 + 5_000);
    expect(ctx.checkForUpdates).not.toHaveBeenCalled();
  });

  it("still allows an explicit manual check when automatic updates are disabled", async () => {
    writeFileSync(
      updaterPreferencesPath(ctx.userDataPath),
      JSON.stringify({ automaticUpdates: false }),
    );
    setupAutoUpdater(() => null);

    await expect(invokeIpc("updater:check")).resolves.toMatchObject({
      ok: true,
    });

    expect(ctx.checkForUpdates).toHaveBeenCalledTimes(1);
  });

  it("forwards update progress to a live renderer", () => {
    const { win, send } = makeWindow();
    setupAutoUpdater(() => win);

    emitUpdater("download-progress", { percent: 42 });

    expect(send).toHaveBeenCalledWith("updater:download-progress", {
      percent: 42,
    });
  });

  it("skips update progress when the BrowserWindow has already been destroyed", () => {
    setupAutoUpdater(() => makeDestroyedWindow());

    expect(() => emitUpdater("download-progress", { percent: 42 })).not.toThrow();
  });

  it("skips update progress when the BrowserWindow webContents has already been destroyed", () => {
    const { win, send } = makeWindowWithDestroyedWebContents();
    setupAutoUpdater(() => win);

    expect(() => emitUpdater("download-progress", { percent: 42 })).not.toThrow();
    expect(send).not.toHaveBeenCalled();
  });

  it("skips update progress when webContents.send loses a destroy race", () => {
    const { win, send } = makeWindowWithThrowingSend(
      new TypeError("Object has been destroyed"),
    );
    setupAutoUpdater(() => win);

    expect(() => emitUpdater("download-progress", { percent: 42 })).not.toThrow();
    expect(send).toHaveBeenCalledWith("updater:download-progress", {
      percent: 42,
    });
  });

  it("rethrows non-destroy errors from webContents.send", () => {
    const { win } = makeWindowWithThrowingSend(new Error("boom"));
    setupAutoUpdater(() => win);

    expect(() => emitUpdater("download-progress", { percent: 42 })).toThrow(
      "boom",
    );
  });
});

/**
 * The updater is a cloud service: it talks to the release feed and writes a
 * downloaded artifact to disk. In local Guest mode neither may happen — the
 * requirement is "not started", not "started and quiet". `index.ts` keeps the
 * module out of Guest boot entirely by importing it lazily; this gate is the
 * second line, covering the window between a cloud logout and teardown.
 */
describe("setupAutoUpdater cloud gate", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    ctx.userDataPath = mkdtempSync(join(tmpdir(), "patchbay-updater-test-"));
    ctx.handlers.clear();
    ctx.ipcHandlers.clear();
    ctx.ipcHandle.mockClear();
    ctx.ipcHandle.mockImplementation((channel: string, handler: IpcHandler) => {
      ctx.ipcHandlers.set(channel, handler);
    });
    ctx.checkForUpdates.mockClear();
    ctx.downloadUpdate.mockClear();
    ctx.quitAndInstall.mockClear();
  });

  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    rmSync(ctx.userDataPath, { recursive: true, force: true });
  });

  it("never contacts the release feed while cloud services are disabled", async () => {
    setupAutoUpdater(() => null, () => false);

    await vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1000);

    expect(ctx.checkForUpdates).not.toHaveBeenCalled();
  });

  it("refuses every updater IPC call while cloud services are disabled", async () => {
    setupAutoUpdater(() => null, () => false);

    await expect(invokeIpc("updater:download")).rejects.toThrow(
      "Cloud services disabled",
    );
    await expect(invokeIpc("updater:install")).rejects.toThrow(
      "Cloud services disabled",
    );
    await expect(invokeIpc("updater:get-preferences")).rejects.toThrow(
      "Cloud services disabled",
    );
    await expect(
      invokeIpc("updater:set-automatic-updates", true),
    ).rejects.toThrow("Cloud services disabled");
    await expect(invokeIpc("updater:check")).resolves.toEqual({
      ok: false,
      error: "Cloud services disabled",
    });

    expect(ctx.downloadUpdate).not.toHaveBeenCalled();
    expect(ctx.quitAndInstall).not.toHaveBeenCalled();
    expect(ctx.checkForUpdates).not.toHaveBeenCalled();
  });

  it("does not push update notifications at a Guest renderer", () => {
    const { win, send } = makeWindow();
    setupAutoUpdater(() => win, () => false);

    emitUpdater("update-available", { version: "9.9.9" });
    emitUpdater("download-progress", { percent: 42 });
    emitUpdater("update-downloaded", { version: "9.9.9" });

    expect(send).not.toHaveBeenCalled();
  });

  it("stops checking and detaches every listener after teardown", async () => {
    const { win, send } = makeWindow();
    const teardown = setupAutoUpdater(() => win, () => true);

    teardown();
    await vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1000);
    emitUpdater("download-progress", { percent: 42 });

    expect(ctx.checkForUpdates).not.toHaveBeenCalled();
    expect(send).not.toHaveBeenCalled();
    expect(ctx.ipcHandlers.has("updater:check")).toBe(false);
  });
});

describe("runMenuUpdateCheck", () => {
  beforeEach(() => {
    ctx.handlers.clear();
    ctx.checkForUpdates.mockReset();
    ctx.checkForUpdates.mockResolvedValue({
      updateInfo: { version: "0.3.18" },
      isUpdateAvailable: false,
    });
    ctx.quitAndInstall.mockClear();
    ctx.getVersion.mockClear();
    ctx.preferredLanguages = ["en-US"];
    ctx.showMessageBox.mockReset();
    ctx.showMessageBox.mockResolvedValue({
      response: 0,
      checkboxChecked: false,
    });
    resetUpdaterTransientStateForTests();
  });

  it("tells the user they are on the latest version", async () => {
    await runMenuUpdateCheck(() => null);

    expect(ctx.checkForUpdates).toHaveBeenCalledTimes(1);
    expect(ctx.showMessageBox).toHaveBeenCalledWith(
      expect.objectContaining({
        type: "info",
        message: "You're up to date",
        detail: "Patchbay 0.3.17 is the latest version.",
      }),
    );
    expect(ctx.quitAndInstall).not.toHaveBeenCalled();
  });

  it("explains that an available version is downloading", async () => {
    ctx.checkForUpdates.mockResolvedValue({
      updateInfo: { version: "0.4.0" },
      isUpdateAvailable: true,
    });

    await runMenuUpdateCheck(() => null);

    expect(ctx.showMessageBox).toHaveBeenCalledWith(
      expect.objectContaining({
        type: "info",
        message: "A new version is available",
        detail:
          "Version 0.4.0 is downloading in the background. You'll be notified when it's ready to install.",
      }),
    );
  });

  it("shows the check error when metadata cannot be fetched", async () => {
    ctx.checkForUpdates.mockRejectedValue(new Error("network down"));

    await runMenuUpdateCheck(() => null);

    expect(ctx.showMessageBox).toHaveBeenCalledWith(
      expect.objectContaining({
        type: "error",
        message: "Couldn't check for updates",
        detail: "network down",
      }),
    );
  });

  it("offers to restart once the package is already downloaded", async () => {
    await runMenuUpdateCheck(() => null);
    emitUpdater("update-downloaded", { version: "0.4.0" });
    ctx.showMessageBox.mockClear();
    ctx.checkForUpdates.mockClear();

    await runMenuUpdateCheck(() => null);

    expect(ctx.checkForUpdates).not.toHaveBeenCalled();
    expect(ctx.showMessageBox).toHaveBeenCalledWith(
      expect.objectContaining({
        message: "Update ready",
        buttons: ["Restart Now", "Later"],
      }),
    );
    expect(ctx.quitAndInstall).toHaveBeenCalledWith(false, true);
  });

  it("leaves the downloaded package in place when the user chooses Later", async () => {
    await runMenuUpdateCheck(() => null);
    emitUpdater("update-downloaded", { version: "0.4.0" });
    ctx.showMessageBox.mockResolvedValue({
      response: 1,
      checkboxChecked: false,
    });

    await runMenuUpdateCheck(() => null);

    expect(ctx.quitAndInstall).not.toHaveBeenCalled();
  });
});
