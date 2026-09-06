import { autoUpdater, type UpdateDownloadedEvent } from "electron-updater";
import { app, dialog, type BrowserWindow, ipcMain } from "electron";
import type {
  ManualUpdateCheckResult,
  UpdaterPreferences,
} from "../shared/updater-types";
import { preferredAppLocaleFromLanguages } from "./os-locale";
import {
  DEFAULT_UPDATER_PREFERENCES,
  loadUpdaterPreferences,
  saveUpdaterPreferences,
  updaterPreferencesPath,
} from "./updater-preferences";

// Silent background updates: electron-updater downloads on its own as soon
// as `update-available` fires; we only surface UI when the package is fully
// downloaded and ready to install on next quit.
autoUpdater.autoDownload = true;
autoUpdater.autoInstallOnAppQuit = true;

// Windows arm64 ships its own update metadata channel because
// electron-builder's `latest.yml` is not arch-suffixed on Windows — both
// arches would otherwise collide on the same file in the GitHub Release.
// See scripts/package.mjs (builderArgsForTarget) for the publish-side half
// of this pact. Pin the channel here so arm64 clients fetch
// `latest-arm64.yml` instead of the x64 metadata.
if (process.platform === "win32" && process.arch === "arm64") {
  autoUpdater.channel = "latest-arm64";
}

interface ChannelConfigurableUpdater {
  channel: string | null;
  allowDowngrade: boolean;
}

export function configureMacX64UpdateChannel(
  updater: ChannelConfigurableUpdater,
  platform: NodeJS.Platform = process.platform,
  arch: string = process.arch,
): void {
  if (platform !== "darwin" || arch !== "x64") return;

  // AppUpdater.channel enables allowDowngrade as a side effect. This channel
  // isolates a CPU architecture, not a release train, so preserve normal
  // monotonic version behavior after selecting the architecture feed.
  updater.channel = "latest-x64";
  updater.allowDowngrade = false;
}

// electron-builder does not architecture-suffix macOS update metadata.
// package.mjs publishes macOS x64 as `latest-x64-mac.yml`; the established
// arm64 feed and runtime path remain unchanged.
configureMacX64UpdateChannel(autoUpdater);

let lastDownloadedVersion: string | null = null;
let trackingDownloadedVersion = false;

function trackDownloadedVersion(): void {
  if (trackingDownloadedVersion) return;
  trackingDownloadedVersion = true;
  autoUpdater.on("update-downloaded", (info) => {
    lastDownloadedVersion = info.version;
  });
}

export function resetUpdaterTransientStateForTests(): void {
  lastDownloadedVersion = null;
  trackingDownloadedVersion = false;
}

async function performManualCheck(): Promise<ManualUpdateCheckResult> {
  try {
    const result = (await checkForUpdatesOnce()) as
      | { updateInfo: { version: string }; isUpdateAvailable?: boolean }
      | null;
    const currentVersion = app.getVersion();
    // Trust electron-updater's own decision rather than re-deriving it from
    // a version-string compare. The two diverge for pre-release channels,
    // staged rollouts, downgrades, and minimum-system-version gates — in
    // those cases updateInfo.version differs from app.getVersion() but no
    // `update-available` event fires, so showing "available" here would
    // promise a download prompt that never appears.
    return {
      ok: true,
      currentVersion,
      latestVersion: result?.updateInfo.version ?? currentVersion,
      available: result?.isUpdateAvailable ?? false,
    };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

type MenuUpdateCopy = {
  upToDate: string;
  upToDateDetail: (version: string) => string;
  updateAvailable: string;
  updateAvailableDetail: (version: string) => string;
  readyMessage: string;
  readyDetail: (version: string) => string;
  restartNow: string;
  later: string;
  checkFailed: string;
  ok: string;
};

const menuUpdateCopyByLocale: Record<
  ReturnType<typeof preferredAppLocaleFromLanguages>,
  MenuUpdateCopy
> = {
  en: {
    upToDate: "You're up to date",
    upToDateDetail: (version) => `Orvilo ${version} is the latest version.`,
    updateAvailable: "A new version is available",
    updateAvailableDetail: (version) =>
      `Version ${version} is downloading in the background. You'll be notified when it's ready to install.`,
    readyMessage: "Update ready",
    readyDetail: (version) =>
      `Version ${version} is ready to install. Restart Orvilo to apply it.`,
    restartNow: "Restart Now",
    later: "Later",
    checkFailed: "Couldn't check for updates",
    ok: "OK",
  },
  "zh-Hans": {
    upToDate: "已是最新版本",
    upToDateDetail: (version) => `当前版本是 ${version}。`,
    updateAvailable: "发现新版本",
    updateAvailableDetail: (version) =>
      `正在后台下载 ${version}，就绪后会通知你。`,
    readyMessage: "更新已就绪",
    readyDetail: (version) => `版本 ${version} 已准备好安装。重启 Orvilo 以完成更新。`,
    restartNow: "立即重启",
    later: "稍后",
    checkFailed: "无法检查更新",
    ok: "好",
  },
  ja: {
    upToDate: "最新バージョンです",
    upToDateDetail: (version) => `現在のバージョンは ${version} です。`,
    updateAvailable: "新しいバージョンがあります",
    updateAvailableDetail: (version) =>
      `バージョン ${version} をバックグラウンドでダウンロードしています。準備ができたら通知します。`,
    readyMessage: "アップデートの準備ができました",
    readyDetail: (version) =>
      `バージョン ${version} をインストールする準備ができました。再起動して適用します。`,
    restartNow: "今すぐ再起動",
    later: "後で",
    checkFailed: "更新を確認できませんでした",
    ok: "OK",
  },
  ko: {
    upToDate: "최신 버전입니다",
    upToDateDetail: (version) => `현재 버전은 ${version}입니다.`,
    updateAvailable: "새 버전이 있습니다",
    updateAvailableDetail: (version) =>
      `${version}을(를) 백그라운드에서 다운로드하고 있습니다. 준비되면 알려 드립니다.`,
    readyMessage: "업데이트를 설치할 준비가 되었습니다",
    readyDetail: (version) =>
      `${version}을(를) 설치할 준비가 되었습니다. 다시 시작하여 적용하세요.`,
    restartNow: "지금 다시 시작",
    later: "나중에",
    checkFailed: "업데이트를 확인할 수 없습니다",
    ok: "확인",
  },
};

function menuUpdateCopy(
  languages: readonly string[] = app.getPreferredSystemLanguages(),
): MenuUpdateCopy {
  return menuUpdateCopyByLocale[preferredAppLocaleFromLanguages(languages)];
}

async function showUpdateDialog(
  window: BrowserWindow | null,
  options: Electron.MessageBoxOptions,
): Promise<Electron.MessageBoxReturnValue> {
  if (window && !window.isDestroyed()) {
    return dialog.showMessageBox(window, options);
  }
  return dialog.showMessageBox(options);
}

/**
 * Native "Check for Updates…" flow for the application menu. Separate from
 * the settings-tab IPC so a menu click still works before Cloud services
 * are enabled, and so the result is a system dialog rather than a toast
 * buried in Settings.
 */
export async function runMenuUpdateCheck(
  getWindow: () => BrowserWindow | null,
): Promise<void> {
  trackDownloadedVersion();
  const copy = menuUpdateCopy();
  const window = getWindow();

  if (lastDownloadedVersion) {
    const { response } = await showUpdateDialog(window, {
      type: "info",
      message: copy.readyMessage,
      detail: copy.readyDetail(lastDownloadedVersion),
      buttons: [copy.restartNow, copy.later],
      defaultId: 0,
      cancelId: 1,
      noLink: true,
    });
    if (response === 0) {
      autoUpdater.quitAndInstall(false, true);
    }
    return;
  }

  const result = await performManualCheck();
  if (!result.ok) {
    await showUpdateDialog(window, {
      type: "error",
      message: copy.checkFailed,
      detail: result.error,
      buttons: [copy.ok],
      defaultId: 0,
      noLink: true,
    });
    return;
  }

  if (!result.available) {
    await showUpdateDialog(window, {
      type: "info",
      message: copy.upToDate,
      detail: copy.upToDateDetail(result.currentVersion),
      buttons: [copy.ok],
      defaultId: 0,
      noLink: true,
    });
    return;
  }

  await showUpdateDialog(window, {
    type: "info",
    message: copy.updateAvailable,
    detail: copy.updateAvailableDetail(result.latestVersion),
    buttons: [copy.ok],
    defaultId: 0,
    noLink: true,
  });
}

const STARTUP_CHECK_DELAY_MS = 5_000;
const PERIODIC_CHECK_INTERVAL_MS = 60 * 60 * 1000; // 1 hour

type RendererChannel =
  | "updater:update-available"
  | "updater:download-progress"
  | "updater:update-downloaded";

function isDestroyedObjectError(err: unknown): boolean {
  return err instanceof Error && err.message.includes("Object has been destroyed");
}

function sendToLiveRenderer(
  win: BrowserWindow | null,
  channel: RendererChannel,
  payload: unknown,
): void {
  if (!win || win.isDestroyed()) return;

  try {
    const { webContents } = win;
    if (webContents.isDestroyed()) return;
    webContents.send(channel, payload);
  } catch (err) {
    if (isDestroyedObjectError(err)) return;
    throw err;
  }
}

// Single-flight guard around checkForUpdates(). With autoDownload=true the
// startup, periodic, and manual triggers can all kick off downloads, and
// overlapping calls have caused duplicate download warnings in the past
// (see electronjs.org/docs/latest/api/auto-updater). Coalesce concurrent
// callers onto the same in-flight promise.
let inFlightCheck: Promise<unknown> | null = null;
function checkForUpdatesOnce(): Promise<unknown> {
  if (inFlightCheck) return inFlightCheck;
  const p = autoUpdater
    .checkForUpdates()
    .then((result) => {
      // checkForUpdates resolves as soon as metadata is fetched; the actual
      // download (when autoDownload=true) is exposed on result.downloadPromise.
      // Without a handler a download failure becomes an unhandled rejection
      // in the main process — Node may terminate it on future versions.
      void (result as { downloadPromise?: Promise<unknown> } | null)?.downloadPromise?.catch(
        (err) => {
          console.error("Failed to download update:", err);
        },
      );
      return result;
    })
    .finally(() => {
      if (inFlightCheck === p) inFlightCheck = null;
    });
  inFlightCheck = p;
  return p;
}

export function setupAutoUpdater(
  getMainWindow: () => BrowserWindow | null,
  isCloudEnabled: () => boolean = () => true,
): () => void {
  const preferencesFilePath = updaterPreferencesPath(app.getPath("userData"));
  let automaticUpdatesEnabled =
    DEFAULT_UPDATER_PREFERENCES.automaticUpdates;
  let startupCheckElapsed = false;
  let startupTimer: ReturnType<typeof setTimeout> | null = null;
  let periodicTimer: ReturnType<typeof setInterval> | null = null;
  let active = true;
  const preferencesReady = loadUpdaterPreferences(preferencesFilePath).then(
    (preferences) => {
      automaticUpdatesEnabled = preferences.automaticUpdates;
      return preferences;
    },
  );

  const runAutomaticCheck = (errorMessage: string): void => {
    void preferencesReady
      .then(() => {
        if (!active || !isCloudEnabled() || !automaticUpdatesEnabled) return;
        return checkForUpdatesOnce();
      })
      .catch((err) => {
        console.error(errorMessage, err);
      });
  };

  // Arm the startup + periodic background checks. Idempotent: an already-armed
  // timer is left in place so re-enabling never stacks duplicate schedules.
  const scheduleBackgroundChecks = (): void => {
    if (!active || !isCloudEnabled()) return;
    if (startupTimer === null && !startupCheckElapsed) {
      // Initial check shortly after startup so we don't block boot.
      startupTimer = setTimeout(() => {
        startupTimer = null;
        startupCheckElapsed = true;
        runAutomaticCheck("Failed to check for updates:");
      }, STARTUP_CHECK_DELAY_MS);
    }
    if (periodicTimer === null) {
      // Background poll so long-running sessions still pick up new releases
      // without requiring the user to restart the app.
      periodicTimer = setInterval(() => {
        runAutomaticCheck("Periodic update check failed:");
      }, PERIODIC_CHECK_INTERVAL_MS);
    }
  };

  // Tear down the scheduled checks outright when automatic updates are turned
  // off. Relying only on an in-callback preference guard leaves the timers
  // running and lets a tick that races the preference flip still fire a check;
  // clearing them makes "disabled" mean no future background work, full stop.
  const cancelBackgroundChecks = (): void => {
    if (startupTimer !== null) {
      clearTimeout(startupTimer);
      startupTimer = null;
    }
    if (periodicTimer !== null) {
      clearInterval(periodicTimer);
      periodicTimer = null;
    }
  };

  const onUpdateAvailable = (info: { version: string; releaseNotes?: unknown }) => {
    if (!active || !isCloudEnabled()) return;
    // Forwarded for renderer-side state tracking only; the notification UI
    // does not render an "available" affordance with autoDownload=true.
    sendToLiveRenderer(getMainWindow(), "updater:update-available", {
      version: info.version,
      releaseNotes: info.releaseNotes,
    });
  };

  const onDownloadProgress = (progress: { percent: number }) => {
    if (!active || !isCloudEnabled()) return;
    sendToLiveRenderer(getMainWindow(), "updater:download-progress", {
      percent: progress.percent,
    });
  };

  const onUpdateDownloaded = (info: UpdateDownloadedEvent) => {
    if (!active || !isCloudEnabled()) return;
    sendToLiveRenderer(getMainWindow(), "updater:update-downloaded", {
      version: info.version,
      releaseNotes: info.releaseNotes,
    });
  };

  const onUpdaterError = (err: unknown) => {
    if (!active || !isCloudEnabled()) return;
    console.error("Auto-updater error:", err);
  };

  trackDownloadedVersion();
  autoUpdater.on("update-available", onUpdateAvailable);
  autoUpdater.on("download-progress", onDownloadProgress);
  autoUpdater.on("update-downloaded", onUpdateDownloaded);
  autoUpdater.on("error", onUpdaterError);

  // Retained for IPC back-compat with older renderer bundles. With
  // autoDownload=true the renderer no longer triggers this path.
  ipcMain.handle("updater:download", () => {
    if (!active || !isCloudEnabled()) throw new Error("Cloud services disabled");
    return autoUpdater.downloadUpdate();
  });

  ipcMain.handle("updater:install", () => {
    if (!active || !isCloudEnabled()) throw new Error("Cloud services disabled");
    autoUpdater.quitAndInstall(false, true);
  });

  ipcMain.handle(
    "updater:get-preferences",
    async (): Promise<UpdaterPreferences> => {
      if (!active || !isCloudEnabled()) throw new Error("Cloud services disabled");
      await preferencesReady;
      return { automaticUpdates: automaticUpdatesEnabled };
    },
  );

  ipcMain.handle(
    "updater:set-automatic-updates",
    async (_event, enabled: unknown): Promise<UpdaterPreferences> => {
      if (!active || !isCloudEnabled()) throw new Error("Cloud services disabled");
      if (typeof enabled !== "boolean") {
        throw new TypeError("automaticUpdates must be a boolean");
      }

      await preferencesReady;
      const wasEnabled = automaticUpdatesEnabled;
      const preferences = { automaticUpdates: enabled };
      await saveUpdaterPreferences(preferencesFilePath, preferences);
      automaticUpdatesEnabled = enabled;

      if (!enabled) {
        cancelBackgroundChecks();
      } else if (!wasEnabled) {
        // If the startup check has already passed while the preference was off,
        // enabling it should take effect now instead of waiting up to one hour.
        if (startupCheckElapsed) {
          runAutomaticCheck("Failed to check for updates:");
        }
        scheduleBackgroundChecks();
      }

      return preferences;
    },
  );

  ipcMain.handle("updater:check", async (): Promise<ManualUpdateCheckResult> => {
    if (!active || !isCloudEnabled()) {
      return { ok: false, error: "Cloud services disabled" };
    }
    return performManualCheck();
  });

  // Initial check shortly after startup so we don't block boot, plus a
  // background poll for long-running sessions. Both are torn down when the
  // user disables automatic updates and re-armed when they turn them back on.
  scheduleBackgroundChecks();

  return () => {
    if (!active) return;
    active = false;
    cancelBackgroundChecks();
    autoUpdater.removeListener("update-available", onUpdateAvailable);
    autoUpdater.removeListener("download-progress", onDownloadProgress);
    autoUpdater.removeListener("update-downloaded", onUpdateDownloaded);
    autoUpdater.removeListener("error", onUpdaterError);
    ipcMain.removeHandler("updater:download");
    ipcMain.removeHandler("updater:install");
    ipcMain.removeHandler("updater:get-preferences");
    ipcMain.removeHandler("updater:set-automatic-updates");
    ipcMain.removeHandler("updater:check");
  };
}
