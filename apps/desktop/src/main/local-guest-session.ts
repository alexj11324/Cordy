import { app, BrowserWindow, ipcMain } from "electron";
import type {
  GuestCloudModeResult,
  GuestCloudTeardownResult,
  LocalGuestMode,
  GuestSessionClearResult,
  GuestSessionMutationResult,
  GuestSessionReadResult,
  LocalGuestSession,
} from "../shared/local-guest";
import { normalizeGuestDisplayName } from "../shared/local-guest";
import {
  clearLocalGuestSession,
  loadLocalGuestSession,
  localGuestSessionPath,
  saveLocalGuestSession,
} from "./local-guest-session-storage";

type MainWindowGetter = () => BrowserWindow | null;
type CloudModeCallback = () => Promise<void> | void;
type CloudTeardownCallback = () => Promise<void> | void;
type ModeChangedCallback = (mode: LocalGuestMode) => void;

export type LocalGuestSessionController = {
  getMode: () => LocalGuestMode;
  enterCloudFromMain: () => Promise<boolean>;
};

function isMainWindowSender(
  event: Electron.IpcMainInvokeEvent,
  getMainWindow: MainWindowGetter,
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

/**
 * Registers the only renderer entry points that can create or transition a
 * local Guest session. The state machine lives in main so a renderer cannot
 * manufacture a cloud user or bypass the explicit Guest → cloud transition.
 */
export async function setupLocalGuestSession(
  getMainWindow: MainWindowGetter,
  onCloudMode: CloudModeCallback,
  onCloudModeDisabled: CloudTeardownCallback,
  onModeChanged: ModeChangedCallback,
): Promise<LocalGuestSessionController> {
  const filePath = localGuestSessionPath(app.getPath("userData"));
  let mode: LocalGuestMode = "undecided";
  let mutationChain: Promise<unknown> = Promise.resolve();

  const initial = await loadLocalGuestSession(filePath);
  if (initial.ok && initial.session) mode = "guest";

  ipcMain.handle("guest-session:mode", (event): LocalGuestMode => {
    return BrowserWindow.fromWebContents(event.sender) ? mode : "undecided";
  });

  const serializeMutation = <T>(operation: () => Promise<T>): Promise<T> => {
    const next = mutationChain.then(operation);
    mutationChain = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  };

  const enterCloud = (): Promise<boolean> =>
    serializeMutation(async () => {
      if (mode === "cloud") return true;
      if (mode === "guest") return false;
      const current = await loadLocalGuestSession(filePath);
      if (!current.ok) return false;
      if (current.session) {
        mode = "guest";
        onModeChanged(mode);
        return false;
      }
      await onCloudMode();
      mode = "cloud";
      onModeChanged(mode);
      return true;
    });

  ipcMain.handle(
    "guest-session:get",
    async (event): Promise<GuestSessionReadResult> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      const result = await loadLocalGuestSession(filePath);
      if (result.ok && result.session && mode !== "guest") {
        mode = "guest";
        onModeChanged(mode);
      }
      return result;
    },
  );

  ipcMain.handle(
    "guest-session:create",
    async (
      event,
      value: unknown,
    ): Promise<GuestSessionMutationResult> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      return serializeMutation(async () => {
        if (mode === "cloud") return { ok: false, reason: "cloud_active" };
        const displayName = normalizeGuestDisplayName(value);
        if (!displayName) return { ok: false, reason: "invalid_name" };

        const current = await loadLocalGuestSession(filePath);
        if (!current.ok) return { ok: false, reason: "unavailable" };
        if (current.session) return { ok: false, reason: "guest_active" };

        const session: LocalGuestSession = { displayName };
        try {
          await saveLocalGuestSession(filePath, session);
        } catch {
          return { ok: false, reason: "unavailable" };
        }
        mode = "guest";
        onModeChanged(mode);
        return { ok: true, session };
      });
    },
  );

  ipcMain.handle(
    "guest-session:clear",
    async (event): Promise<GuestSessionClearResult> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      return serializeMutation(async () => {
        if (mode === "cloud") return { ok: false, reason: "cloud_active" };
        try {
          await clearLocalGuestSession(filePath);
        } catch {
          return { ok: false, reason: "unavailable" };
        }
        mode = "undecided";
        onModeChanged(mode);
        return { ok: true };
      });
    },
  );

  ipcMain.handle(
    "guest-session:enable-cloud",
    async (event): Promise<GuestCloudModeResult> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      return serializeMutation(async () => {
        if (mode === "cloud") return { ok: true };
        if (mode === "guest") return { ok: false, reason: "guest_active" };

        const current = await loadLocalGuestSession(filePath);
        if (!current.ok) return { ok: false, reason: "unavailable" };
        if (current.session) {
          mode = "guest";
          onModeChanged(mode);
          return { ok: false, reason: "guest_active" };
        }

        await onCloudMode();
        mode = "cloud";
        onModeChanged(mode);
        return { ok: true };
      });
    },
  );

  ipcMain.handle(
    "guest-session:switch-to-cloud",
    async (event): Promise<GuestCloudModeResult> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      return serializeMutation(async () => {
        if (mode === "cloud") return { ok: true };
        const current = await loadLocalGuestSession(filePath);
        if (!current.ok) return { ok: false, reason: "unavailable" };
        if (!current.session) {
          mode = "undecided";
          return { ok: false, reason: "no_guest" };
        }

        try {
          await clearLocalGuestSession(filePath);
        } catch {
          return { ok: false, reason: "unavailable" };
        }
        mode = "cloud";
        await onCloudMode();
        onModeChanged(mode);
        return { ok: true };
      });
    },
  );

  ipcMain.handle(
    "guest-session:disable-cloud",
    async (event): Promise<GuestCloudTeardownResult> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      return serializeMutation(async () => {
        if (mode !== "cloud") return { ok: false, reason: "not_cloud" };
        try {
          await onCloudModeDisabled();
        } catch {
          return { ok: false, reason: "unavailable" };
        }
        mode = "undecided";
        onModeChanged(mode);
        return { ok: true };
      });
    },
  );

  return { getMode: () => mode, enterCloudFromMain: enterCloud };
}
