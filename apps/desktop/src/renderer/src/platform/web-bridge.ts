import { runtimeConfigFromDevEnv } from "../../../shared/runtime-config";
import type { DaemonPrefs, DaemonStatus, LocalRuntimeProbe } from "../../../shared/daemon-types";
import type { NavigationGesture } from "../../../shared/navigation-gestures";
import type { IssueWindowRequest } from "../../../shared/issue-window";

const BROWSER_RENDERER_ERROR =
  "This Desktop control is unavailable in the browser renderer.";

const BROWSER_DAEMON_PREFS: DaemonPrefs = {
  autoStart: false,
  autoStop: false,
};

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
    // A malformed external URL should not break the browser renderer.
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

const viteEnv = import.meta.env as ImportMetaEnv & {
  readonly NEXT_PUBLIC_API_URL?: string;
  readonly NEXT_PUBLIC_WS_URL?: string;
};

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

  const runtimeConfig = runtimeConfigFromDevEnv({
    // Complete `pnpm dev` exports the Next.js convention, while direct
    // Desktop development uses Vite's convention. Accept both so a linked
    // worktree never falls back to the primary checkout's port.
    apiUrl: viteEnv.VITE_API_URL || viteEnv.NEXT_PUBLIC_API_URL,
    wsUrl: viteEnv.VITE_WS_URL || viteEnv.NEXT_PUBLIC_WS_URL,
    appUrl: viteEnv.VITE_APP_URL,
    accountsUrl: viteEnv.VITE_ACCOUNTS_URL,
  });
  const daemonStatus = browserDaemonStatus(runtimeConfig.apiUrl);

  const desktopAPI = {
    /** Identifies the browser host for native-only capability decisions. */
    host: "browser" as const,
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
    // Browser Vite hosts have no native deep-link receiver. The canonical
    // handoff is delivered only by Electron's main/preload bridge through
    // patchbay://auth/callback; this host must never accept an HTTP callback.
    onAuthHandoff: (
      _callback: (payload: { code: string; state: string }) => boolean | Promise<boolean>,
    ) => noopUnsubscribe(),
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
    start: async () => ({ success: false, error: BROWSER_RENDERER_ERROR }),
    stop: async () => ({ success: false, error: BROWSER_RENDERER_ERROR }),
    restart: async () => ({ success: false, error: BROWSER_RENDERER_ERROR }),
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
      message: BROWSER_RENDERER_ERROR,
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
    onLogReset: (_callback: () => void) => noopUnsubscribe(),
    openLogFile: async () => ({
      success: false,
      error: BROWSER_RENDERER_ERROR,
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
  return true;
}
