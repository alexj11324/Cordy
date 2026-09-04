// @vitest-environment node

import { mkdtemp, readFile, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";
import type { LocalGuestMode } from "../shared/local-guest";

type IpcHandler = (...args: unknown[]) => unknown;

const ctx = vi.hoisted(() => ({
  userDataPath: "",
  ipcHandlers: new Map<string, (...args: unknown[]) => unknown>(),
  windowsBySender: new Map<unknown, unknown>(),
}));

vi.mock("electron", () => ({
  app: { getPath: vi.fn(() => ctx.userDataPath) },
  BrowserWindow: {
    fromWebContents: (sender: unknown) =>
      ctx.windowsBySender.get(sender) ?? null,
  },
  ipcMain: {
    handle: (channel: string, handler: IpcHandler) => {
      ctx.ipcHandlers.set(channel, handler);
    },
  },
}));

import { localGuestSessionPath } from "./local-guest-session-storage";
import { setupLocalGuestSession } from "./local-guest-session";

const temporaryDirectories: string[] = [];
const mainSender = { id: "main" };
const otherSender = { id: "other" };
const mainWindow = { isDestroyed: () => false };

let onCloudMode: Mock<() => Promise<void>>;
let onCloudModeDisabled: Mock<() => Promise<void>>;
let modeChanges: LocalGuestMode[];

function invoke(channel: string, sender: unknown, value?: unknown) {
  const handler = ctx.ipcHandlers.get(channel);
  if (!handler) throw new Error(`no handler registered for ${channel}`);
  return handler({ sender }, value);
}

async function writePersistedSession(displayName: string): Promise<void> {
  const filePath = localGuestSessionPath(ctx.userDataPath);
  await mkdir(dirname(filePath), { recursive: true, mode: 0o700 });
  await writeFile(filePath, JSON.stringify({ displayName }), { mode: 0o600 });
}

async function setup() {
  return setupLocalGuestSession(
    () => mainWindow as never,
    onCloudMode,
    onCloudModeDisabled,
    (mode) => modeChanges.push(mode),
  );
}

beforeEach(async () => {
  ctx.ipcHandlers.clear();
  ctx.windowsBySender.clear();
  ctx.windowsBySender.set(mainSender, mainWindow);
  const directory = await mkdtemp(join(tmpdir(), "patchbay-guest-session-"));
  temporaryDirectories.push(directory);
  ctx.userDataPath = directory;
  onCloudMode = vi.fn<() => Promise<void>>(async () => {});
  onCloudModeDisabled = vi.fn<() => Promise<void>>(async () => {});
  modeChanges = [];
});

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

describe("main-owned Guest/cloud mode", () => {
  it("starts undecided and only enters cloud when main says so", async () => {
    const controller = await setup();

    expect(controller.getMode()).toBe("undecided");
    expect(onCloudMode).not.toHaveBeenCalled();

    await expect(invoke("guest-session:enable-cloud", mainSender)).resolves.toEqual(
      { ok: true },
    );
    expect(controller.getMode()).toBe("cloud");
    expect(onCloudMode).toHaveBeenCalledTimes(1);
  });

  it("boots straight into Guest when a Guest session is on disk", async () => {
    await writePersistedSession("Alice");

    const controller = await setup();

    expect(controller.getMode()).toBe("guest");
    // Cloud services must not have been started to discover this.
    expect(onCloudMode).not.toHaveBeenCalled();
  });

  it("never starts cloud services while a Guest session exists", async () => {
    // The whole gate in one assertion: no renderer request and no main-side
    // deep-link promotion may boot the cloud stack for a Guest user.
    await writePersistedSession("Alice");
    const controller = await setup();

    await expect(
      invoke("guest-session:enable-cloud", mainSender),
    ).resolves.toEqual({ ok: false, reason: "guest_active" });
    await expect(controller.enterCloudFromMain()).resolves.toBe(false);

    expect(onCloudMode).not.toHaveBeenCalled();
    expect(controller.getMode()).toBe("guest");
  });

  it("refuses to create a Guest session once cloud mode is live", async () => {
    const controller = await setup();
    await invoke("guest-session:enable-cloud", mainSender);

    await expect(
      invoke("guest-session:create", mainSender, "Alice"),
    ).resolves.toEqual({ ok: false, reason: "cloud_active" });
    expect(controller.getMode()).toBe("cloud");
    // Nothing was persisted, so a later boot does not resurrect a Guest.
    await expect(
      readFile(localGuestSessionPath(ctx.userDataPath), "utf8"),
    ).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("rejects every mutation from a window that is not the main window", async () => {
    await setup();

    for (const channel of [
      "guest-session:get",
      "guest-session:create",
      "guest-session:clear",
      "guest-session:enable-cloud",
      "guest-session:switch-to-cloud",
      "guest-session:disable-cloud",
    ]) {
      await expect(invoke(channel, otherSender, "Alice")).resolves.toEqual({
        ok: false,
        reason: "unauthorized",
      });
    }
    expect(onCloudMode).not.toHaveBeenCalled();
  });

  it("refuses a display name the renderer did not normalize", async () => {
    const controller = await setup();

    for (const name of ["", "   ", "Alice\n", "a".repeat(65), 42, null]) {
      await expect(
        invoke("guest-session:create", mainSender, name),
      ).resolves.toEqual({ ok: false, reason: "invalid_name" });
    }
    expect(controller.getMode()).toBe("undecided");
  });

  it("serializes concurrent creates so only one Guest session wins", async () => {
    await setup();

    const [first, second] = await Promise.all([
      invoke("guest-session:create", mainSender, "Alice"),
      invoke("guest-session:create", mainSender, "Mallory"),
    ]);

    const results = [first, second] as Array<{ ok: boolean; reason?: string }>;
    expect(results.filter((result) => result.ok)).toHaveLength(1);
    expect(results.find((result) => !result.ok)?.reason).toBe("guest_active");
  });

  it("erases the Guest session before it hands control to cloud", async () => {
    await writePersistedSession("Alice");
    const controller = await setup();

    await expect(
      invoke("guest-session:switch-to-cloud", mainSender),
    ).resolves.toEqual({ ok: true });

    expect(controller.getMode()).toBe("cloud");
    expect(onCloudMode).toHaveBeenCalledTimes(1);
    // No Guest identity survives into the cloud session.
    await expect(
      readFile(localGuestSessionPath(ctx.userDataPath), "utf8"),
    ).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("tears cloud services down on logout and returns to undecided", async () => {
    const controller = await setup();
    await invoke("guest-session:enable-cloud", mainSender);

    await expect(
      invoke("guest-session:disable-cloud", mainSender),
    ).resolves.toEqual({ ok: true });

    expect(onCloudModeDisabled).toHaveBeenCalledTimes(1);
    expect(controller.getMode()).toBe("undecided");
    expect(modeChanges.at(-1)).toBe("undecided");
  });

  it("refuses to tear down cloud services that were never started", async () => {
    await setup();

    await expect(
      invoke("guest-session:disable-cloud", mainSender),
    ).resolves.toEqual({ ok: false, reason: "not_cloud" });
    expect(onCloudModeDisabled).not.toHaveBeenCalled();
  });

  it("fails closed and reports Guest when the session file is corrupt", async () => {
    const filePath = localGuestSessionPath(ctx.userDataPath);
    await mkdir(dirname(filePath), { recursive: true, mode: 0o700 });
    await writeFile(filePath, "{not json", { mode: 0o600 });

    const controller = await setup();

    await expect(invoke("guest-session:get", mainSender)).resolves.toEqual({
      ok: false,
      reason: "invalid",
    });
    // An unreadable Guest file must not be treated as "no Guest, go cloud".
    await expect(
      invoke("guest-session:enable-cloud", mainSender),
    ).resolves.toEqual({ ok: false, reason: "unavailable" });
    expect(onCloudMode).not.toHaveBeenCalled();
    expect(controller.getMode()).toBe("undecided");
  });

  it("reports mode changes so main can re-gate every renderer", async () => {
    await setup();

    await invoke("guest-session:create", mainSender, "Alice");
    await invoke("guest-session:clear", mainSender);
    await invoke("guest-session:enable-cloud", mainSender);

    expect(modeChanges).toEqual(["guest", "undecided", "cloud"]);
  });
});
