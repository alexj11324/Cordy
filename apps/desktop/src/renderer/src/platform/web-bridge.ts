import { runtimeConfigFromDevEnv } from "../../../shared/runtime-config";
import type { DaemonPrefs, DaemonStatus, LocalRuntimeProbe } from "../../../shared/daemon-types";
import type { NavigationGesture } from "../../../shared/navigation-gestures";
import type { IssueWindowRequest } from "../../../shared/issue-window";

declare global {
  interface Window {
    /** True when the Desktop renderer is running in Vite's browser host. */
    __PATCHBAY_VITE_DESKTOP_PREVIEW__?: boolean;
    /** True for both fixture and backend-enabled Vite browser hosts. */
    __PATCHBAY_VITE_DESKTOP_HOST__?: boolean;
  }
}

const BROWSER_PREVIEW_ERROR =
  "This Desktop control is unavailable in the browser renderer.";

const BROWSER_DAEMON_PREFS: DaemonPrefs = {
  autoStart: false,
  autoStop: false,
};

const AUTH_CALLBACK_PATH = "/auth/callback";
const HANDOFF_CODE_PATTERN = /^pbd_[A-Za-z0-9_-]{43}$/;
const HANDOFF_STATE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/;

type BrowserAuthHandoff = { code: string; state: string };

function takeBrowserAuthHandoff(): BrowserAuthHandoff | null {
  if (window.location.pathname !== AUTH_CALLBACK_PATH) return null;
  const params = new URLSearchParams(window.location.search);
  const code = params.get("code") ?? "";
  const state = params.get("state") ?? "";
  // Clear every callback query, including malformed attacker-supplied input,
  // before deciding whether it is eligible for redemption.
  window.history.replaceState(null, "", "/");
  if (!HANDOFF_CODE_PATTERN.test(code) || !HANDOFF_STATE_PATTERN.test(state)) {
    return null;
  }

  // The normal App handoff listener redeems it over HTTPS with PKCE.
  return { code, state };
}

function browserPlatform(): "macos" | "windows" | "linux" | "unknown" {
  const platform = navigator.platform.toLowerCase();
  if (platform.includes("mac")) return "macos";
  if (platform.includes("win")) return "windows";
  if (platform.includes("linux")) return "linux";
  return "unknown";
}

function openBrowserUrl(value: string): void {
  try {
    const url = new URL(value, window.location.origin);
    if (url.protocol !== "http:" && url.protocol !== "https:") return;
    window.open(url.toString(), "_blank", "noopener,noreferrer");
  } catch {
    // A malformed external URL should not break the renderer preview.
  }
}

function listenForShortcut(
  key: string,
  callback: () => void,
): () => void {
  const handler = (event: KeyboardEvent) => {
    if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== key) {
      return;
    }
    event.preventDefault();
    callback();
  };
  window.addEventListener("keydown", handler);
  return () => window.removeEventListener("keydown", handler);
}

function noopUnsubscribe(): () => void {
  return () => undefined;
}

function browserDaemonStatus(apiUrl: string): DaemonStatus {
  return {
    state: "stopped",
    profile: "browser",
    serverUrl: apiUrl,
  };
}

/**
 * Install the small browser equivalent of Electron's preload bridge.
 *
 * The Desktop React tree is intentionally shared between Electron and Vite.
 * Electron supplies the real bridge; Vite supplies safe no-op implementations
 * for native-only capabilities so the renderer can be designed in a normal
 * browser without starting a second Next.js application.
 */
export function installWebDesktopBridge(): boolean {
  if (typeof window === "undefined") return false;

  const existingWindow = window as unknown as { desktopAPI?: unknown };
  if (existingWindow.desktopAPI) return false;

  // The documented backend-enabled Vite path is selected by VITE_API_URL.
  // VITE_DESKTOP_PREVIEW remains an explicit fixture-only override when no
  // backend URL is present.
  const preview =
    !import.meta.env.VITE_API_URL &&
    import.meta.env.VITE_DESKTOP_PREVIEW !== "false";
  let pendingBrowserAuthHandoff = preview ? null : takeBrowserAuthHandoff();
  const runtimeConfig = runtimeConfigFromDevEnv({
    // The browser preview owns a same-origin local API middleware by default.
    // An explicit VITE_API_URL still opts into a real remote/local backend for
    // full server-backed acceptance without changing the Vite host.
    apiUrl:
      import.meta.env.VITE_API_URL ||
      (preview ? window.location.origin : undefined),
    wsUrl: import.meta.env.VITE_WS_URL,
    // The no-backend preview must stay on its Vite origin. A backend-enabled
    // browser host derives share links from VITE_API_URL unless VITE_APP_URL is
    // explicit. Auth reuses that app origin unless VITE_ACCOUNTS_URL separates
    // the broker onto another deployed host.
    appUrl:
      import.meta.env.VITE_APP_URL ||
      (preview ? window.location.origin : undefined),
    accountsUrl:
      import.meta.env.VITE_ACCOUNTS_URL ||
      (preview ? window.location.origin : undefined),
  });
  const daemonStatus = browserDaemonStatus(runtimeConfig.apiUrl);

  const desktopAPI = {
    appInfo: {
      version: import.meta.env.VITE_APP_VERSION || "vite",
      os: browserPlatform(),
    },
    systemLocale: navigator.language || "en",
    onSystemLocaleChanged: (callback: (locale: string) => void) => {
      const handler = () => callback(navigator.language || "en");
      window.addEventListener("languagechange", handler);
      return () => window.removeEventListener("languagechange", handler);
    },
    runtimeConfig: { ok: true, config: runtimeConfig },
    windowContext: { kind: "main" as const },
    getLastFreeze: () => null,
    ackFreeze: (_ts: number) => undefined,
    reportAuthSession: (_userId: string | null) => undefined,
    onAuthHandoff: (
      callback: (payload: {
        code: string;
        state: string;
      }) => boolean | Promise<boolean>,
    ) => {
      if (!pendingBrowserAuthHandoff) return noopUnsubscribe();
      let cancelled = false;
      let deliveryInFlight = false;
      const deliver = async () => {
        const pending = pendingBrowserAuthHandoff;
        if (!pending || cancelled || deliveryInFlight) return;
        deliveryInFlight = true;
        try {
          const acknowledged = await callback(pending);
          if (acknowledged && pendingBrowserAuthHandoff === pending) {
            pendingBrowserAuthHandoff = null;
            window.removeEventListener("online", retry);
          }
        } catch {
          // Keep the PKCE-bound code for an explicit browser-online retry.
        } finally {
          deliveryInFlight = false;
        }
      };
      const retry = () => void deliver();
      window.addEventListener("online", retry);
      queueMicrotask(retry);
      return () => {
        cancelled = true;
        window.removeEventListener("online", retry);
      };
    },
    onInviteOpen: (_callback: (invitationId: string) => void) =>
      noopUnsubscribe(),
    openExternal: async (url: string) => openBrowserUrl(url),
    downloadURL: async (url: string) => openBrowserUrl(url),
    setImmersiveMode: async (_immersive: boolean) => undefined,
    showNotification: (payload: {
      slug: string;
      itemId: string;
      issueKey: string;
      title: string;
      body: string;
    }) => {
      if (
        typeof Notification !== "undefined" &&
        Notification.permission === "granted"
      ) {
        new Notification(payload.title, { body: payload.body });
      }
    },
    setUnreadBadge: (_count: number) => undefined,
    onInboxOpen: (
      _callback: (payload: {
        slug: string;
        itemId: string;
        issueKey: string;
      }) => void,
    ) => noopUnsubscribe(),
    onNavigationGesture: (_callback: (gesture: NavigationGesture) => void) =>
      noopUnsubscribe(),
    setRendererRouteContext: (_context: unknown) => undefined,
    pickDirectory: async (_defaultPath?: string) => ({
      ok: false,
      reason: "unsupported",
    }),
    validateLocalDirectory: async (_path: string) => ({
      ok: false,
      reason: "unsupported",
    }),
    onCloseActiveTab: (callback: () => void) =>
      listenForShortcut("w", callback),
    onOpenSettings: (callback: () => void) =>
      listenForShortcut(",", callback),
    closeWindow: () => window.close(),
    openIssueWindow: async (request: IssueWindowRequest) => {
      if (!request.path.startsWith("/") || request.path.startsWith("//")) {
        return { ok: false as const, reason: "invalid_request" as const };
      }
      openBrowserUrl(new URL(request.path, window.location.origin).toString());
      return { ok: true as const };
    },
  } as unknown as Window["desktopAPI"];

  const daemonAPI = {
    start: async () => ({ success: false, error: BROWSER_PREVIEW_ERROR }),
    stop: async () => ({ success: false, error: BROWSER_PREVIEW_ERROR }),
    restart: async () => ({ success: false, error: BROWSER_PREVIEW_ERROR }),
    getStatus: async () => daemonStatus,
    probeRuntimes: async (): Promise<LocalRuntimeProbe> => ({
      probeResult: "error",
    }),
    getHostName: async () => "Browser",
    onStatusChange: (
      _callback: (status: DaemonStatus) => void,
    ) => noopUnsubscribe(),
    setTargetApiUrl: async (_url: string) => undefined,
    syncToken: async (_token: string, _userId: string) => undefined,
    clearToken: async () => undefined,
    reauthenticate: async (_token: string, _userId: string) => ({
      ok: false as const,
      reason: "transient" as const,
      message: BROWSER_PREVIEW_ERROR,
    }),
    isCliInstalled: async () => false,
    getPrefs: async () => BROWSER_DAEMON_PREFS,
    setPrefs: async (prefs: Partial<DaemonPrefs>) => ({
      ...BROWSER_DAEMON_PREFS,
      ...prefs,
    }),
    autoStart: async () => undefined,
    retryInstall: async () => undefined,
    startLogStream: () => undefined,
    stopLogStream: () => undefined,
    onLogLine: (_callback: (line: string) => void) => noopUnsubscribe(),
    openLogFile: async () => ({
      success: false,
      error: BROWSER_PREVIEW_ERROR,
    }),
  } as unknown as Window["daemonAPI"];

  const updater = {
    onUpdateAvailable: (
      _callback: (info: { version: string; releaseNotes?: string }) => void,
    ) => noopUnsubscribe(),
    onDownloadProgress: (_callback: (progress: { percent: number }) => void) =>
      noopUnsubscribe(),
    onUpdateDownloaded: (
      _callback: (info: { version: string; releaseNotes?: string }) => void,
    ) => noopUnsubscribe(),
    downloadUpdate: async () => undefined,
    installUpdate: async () => undefined,
    getPreferences: async () => ({ automaticUpdates: false }),
    setAutomaticUpdates: async (enabled: boolean) => ({
      automaticUpdates: enabled,
    }),
    checkForUpdates: async () => ({
      ok: true as const,
      currentVersion: "vite",
      latestVersion: "vite",
      available: false,
    }),
  } as unknown as Window["updater"];

  window.desktopAPI = desktopAPI;
  window.daemonAPI = daemonAPI;
  window.updater = updater;
  window.electron = {} as Window["electron"];
  window.__PATCHBAY_VITE_DESKTOP_HOST__ = true;
  window.__PATCHBAY_VITE_DESKTOP_PREVIEW__ = preview;
  return true;
}

export function isDesktopWebHost(): boolean {
  return (
    typeof window !== "undefined" &&
    window.__PATCHBAY_VITE_DESKTOP_HOST__ === true
  );
}

export function isDesktopWebPreview(): boolean {
  return (
    typeof window !== "undefined" &&
    window.__PATCHBAY_VITE_DESKTOP_PREVIEW__ === true
  );
}
