import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { CoreProvider } from "@patchbay/core/platform";
import { pickLocale, type SupportedLocale } from "@patchbay/core/i18n";
import { useAuthStore } from "@patchbay/core/auth";
import { useWelcomeStore } from "@patchbay/core/onboarding";
import { workspaceKeys } from "@patchbay/core/workspace/queries";
import { useWorkspaceList } from "@patchbay/core/workspace";
import { api } from "@patchbay/core/api";
import { useHasOnboarded } from "@patchbay/core/paths";
import { setCurrentWorkspace } from "@patchbay/core/platform";
import { ThemeProvider } from "@patchbay/ui/components/common/theme-provider";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { Toaster } from "@patchbay/ui/components/ui/sonner";
import { DesktopLoginPage } from "./pages/login";
import { DesktopAuthRecoveryPage } from "./pages/auth-recovery";
import { DesktopShell } from "./components/desktop-layout";
import { UpdateNotification } from "./components/update-notification";
import { IssueWindow } from "./components/issue-window";
import { useTabStore } from "./stores/tab-store";
import { useWindowOverlayStore } from "./stores/window-overlay-store";
import { useOpenSettingsShortcut } from "./hooks/use-open-settings-shortcut";
import { useDaemonIPCBridge } from "./platform/daemon-ipc-bridge";
import { syncDaemonOnLogin } from "./platform/daemon-login-sync";
import { createDesktopLocaleAdapter } from "./platform/i18n-adapter";
import { captureEvent } from "@patchbay/core/analytics";
import { RESOURCES } from "@patchbay/views/locales";
import { DesktopClientUsageReporter } from "./platform/client-usage-reporter";
import { DiagnosticRouteReporter } from "./platform/diagnostic-route-reporter";
import { flushFreezeBreadcrumb } from "./freeze-flush";
import { DesktopAuthSessionBridge } from "./platform/auth-session-bridge";
import { completeDesktopHandoff } from "./pages/login-handoff";
import type { DaemonRecoveryReason } from "../../shared/daemon-types";

// BCP-47 region tags for the <html lang> attribute, mirroring
// apps/web/app/layout.tsx HTML_LANG. index.html ships a static lang="en";
// we sync it to the resolved locale at boot so screen readers announce the
// right language AND the Japanese-scoped CJK font override in globals.css
// (`html[lang|="ja"]`) can take effect.
const HTML_LANG: Record<SupportedLocale, string> = {
  en: "en",
  "zh-Hans": "zh-CN",
  ko: "ko-KR",
  ja: "ja-JP",
};


/**
 * Cmd/Ctrl+W: close the active tab. When the last real tab is closed
 * (or no tabs/workspace exist — e.g. login page), close the window.
 *
 * Mounted at the App root so every renderer state — including login,
 * loading, onboarding, and runtime-config errors — has a working Cmd+W
 * handler. Without this, states outside the tab shell would swallow the
 * shortcut and do nothing.
 */
function useCmdWCloseTab() {
  useEffect(() => {
    return window.desktopAPI.onCloseActiveTab(() => {
      if (window.desktopAPI.windowContext?.kind === "issue") {
        window.desktopAPI.closeWindow();
        return;
      }
      const overlay = useWindowOverlayStore.getState();
      if (overlay.overlay?.type === "settings") {
        overlay.close();
        return;
      }
      const store = useTabStore.getState();
      const { activeWorkspaceSlug, byWorkspace } = store;
      if (!activeWorkspaceSlug) {
        // No workspace — nothing to close, dismiss the window.
        window.desktopAPI.closeWindow();
        return;
      }
      const group = byWorkspace[activeWorkspaceSlug];
      if (!group || group.tabs.length <= 1) {
        // Last tab (or no tabs) — close the window.
        window.desktopAPI.closeWindow();
        return;
      }
      // Multiple tabs — close the active one.
      store.closeActiveTab();
    });
  }, []);
}

function IssueWindowContent() {
  const user = useAuthStore((state) => state.user);
  const isLoading = useAuthStore((state) => state.isLoading);
  const authStatus = useAuthStore((state) => state.status);
  const context = window.desktopAPI.windowContext ?? { kind: "main" as const };

  if (context.kind !== "issue") return null;
  if (authStatus === "recovering") return <DesktopAuthRecoveryPage />;
  if (isLoading) {
    return (
      <div className="flex h-screen items-center justify-center">
        <PatchbayIcon className="size-6 animate-pulse" />
      </div>
    );
  }

  return user ? <IssueWindow context={context} /> : <DesktopLoginPage />;
}

export function AppContent() {
  const user = useAuthStore((s) => s.user);
  const isLoading = useAuthStore((s) => s.isLoading);
  const authStatus = useAuthStore((s) => s.status);
  const qc = useQueryClient();
  const isElectronRenderer = window.desktopAPI.host === "electron";

  // Deep-link login runs loginWithToken → syncToken → listWorkspaces →
  // setQueryData sequentially. loginWithToken sets user+isLoading=false
  // as soon as getMe resolves, which would cause DesktopShell to mount
  // before the workspace list is hydrated and briefly see `!workspace`.
  // This local flag keeps the loading screen up until the whole chain
  // finishes, so IndexRedirect gets a definitive workspace state on
  // first render.
  const [bootstrapping, setBootstrapping] = useState(false);
  const [daemonSyncRetry, setDaemonSyncRetry] = useState(0);
  const [daemonSyncState, setDaemonSyncState] = useState<
    "idle" | "pending" | "ready" | "error"
  >(isElectronRenderer ? "pending" : "ready");
  const [daemonSyncError, setDaemonSyncError] =
    useState<DaemonRecoveryReason | null>(null);
  // A ready state is only valid for the exact authenticated identity and
  // backend target that produced it. Keeping the key beside the state closes
  // the render/effect gap during account or target switches: the previous
  // account's ready state cannot mount DesktopShell for even one frame.
  const [daemonSyncedKey, setDaemonSyncedKey] = useState<string | null>(null);
  const daemonSyncGeneration = useRef(0);

  const runtimeConfig = window.desktopAPI.runtimeConfig.ok
    ? window.desktopAPI.runtimeConfig.config
    : null;
  const daemonIdentityKey =
    user && runtimeConfig ? `${user.id}\u0000${runtimeConfig.apiUrl}` : null;

  // Tell the main process which backend URL we talk to, so daemon-manager
  // can pick the matching CLI profile (server_url from ~/.patchbay config).
  useEffect(() => {
    if (!runtimeConfig) return;
    window.daemonAPI.setTargetApiUrl(runtimeConfig.apiUrl);
  }, [runtimeConfig]);

  // Listen for invite IDs delivered via deep link (patchbay://invite/<id>).
  // We open the overlay regardless of login state — if the user isn't logged
  // in, InvitePage's queries will fail and render the "not found" state,
  // which is acceptable; the expected pre-flight happens in the web app
  // (login + next=/invite/... dance) before the deep link is ever dispatched.
  useEffect(() => {
    return window.desktopAPI.onInviteOpen((invitationId) => {
      useWindowOverlayStore.getState().open({ type: "invite", invitationId });
    });
  }, []);

  // Listen for the PKCE-bound one-time code delivered by the Electron deep
  // link. daemonAPI.syncToken is handled
  // separately by the [user] effect below, which fires whenever a user logs
  // in (handoff, session restore, account switch).
  useEffect(() => {
    return window.desktopAPI.onAuthHandoff(async ({ code, state }) => {
      setBootstrapping(true);
      let acknowledged = false;
      try {
        const completion = await completeDesktopHandoff(code, state, {
          redeem: (handoffCode, verifier) =>
            api.redeemDesktopHandoff(handoffCode, verifier),
          login: (token) => useAuthStore.getState().loginWithToken(token),
          recoverPersistedToken: () =>
            useAuthStore.getState().retryAuthentication(),
        });
        acknowledged = completion.acknowledged;
        if (!completion.authenticated) return completion.acknowledged;
        // Seed React Query cache with the workspace list so the index-route
        // redirect (routes.tsx `IndexRedirect`) can resolve the initial
        // destination without a second fetch. Workspace side-effects
        // (setCurrentWorkspace, persist namespace) are synced later by
        // WorkspaceRouteLayout when the URL resolves.
        const wsList = await api.listWorkspaces();
        qc.setQueryData(workspaceKeys.list(), wsList);
        return completion.acknowledged;
      } catch {
        // Token invalid or expired — user stays on login page
        return acknowledged;
      } finally {
        setBootstrapping(false);
      }
    });
  }, [qc]);

  // Sync token and start the daemon whenever the user logs in. The ordering
  // inside syncDaemonOnLogin is load-bearing — see that module.
  useEffect(() => {
    const generation = ++daemonSyncGeneration.current;
    if (!user) {
      setDaemonSyncState("idle");
      setDaemonSyncError(null);
      setDaemonSyncedKey(null);
      return;
    }
    if (!isElectronRenderer || !runtimeConfig) {
      setDaemonSyncState("ready");
      setDaemonSyncError(null);
      setDaemonSyncedKey(daemonIdentityKey);
      return;
    }
    const token = localStorage.getItem("patchbay_token");
    if (!token) {
      // Token-mode Desktop sessions must have a renderer session token so the
      // main process can mint the Desktop-owned daemon PAT. Do not mount the
      // shell with a daemon that still belongs to another account.
      setDaemonSyncState("error");
      setDaemonSyncError("session_token_missing");
      setDaemonSyncedKey(daemonIdentityKey);
      return;
    }
    const userId = user.id;
    setDaemonSyncState("pending");
    setDaemonSyncError(null);
    setDaemonSyncedKey(null);
    (async () => {
      try {
        await syncDaemonOnLogin(
          window.daemonAPI,
          runtimeConfig.apiUrl,
          token,
          userId,
        );
        if (generation === daemonSyncGeneration.current) {
          setDaemonSyncState("ready");
          setDaemonSyncedKey(daemonIdentityKey);
        }
      } catch (err) {
        console.error("Failed to sync daemon on login", err);
        if (generation === daemonSyncGeneration.current) {
          setDaemonSyncState("error");
          const reason = (err as { reason?: unknown })?.reason;
          setDaemonSyncError(
            reason === "auto_start_disabled" ||
              reason === "cli_not_found" ||
              reason === "auth_expired" ||
              reason === "start_failed" ||
              reason === "not_ready"
              ? reason
              : "start_failed",
          );
          setDaemonSyncedKey(daemonIdentityKey);
        }
      }
    })();
    return () => undefined;
  }, [
    user,
    runtimeConfig,
    daemonIdentityKey,
    isElectronRenderer,
    daemonSyncRetry,
  ]);

  // When a user who started the session with zero workspaces creates their
  // first one, restart the daemon so it picks up the new workspace
  // immediately (otherwise workspaceSyncLoop's next 30s tick would be the
  // earliest pickup point). Specifically scoped to "started empty" because
  // account switches (user A logout → user B login) should not trigger a
  // daemon restart here — daemon-manager already restarts on user change
  // via syncToken.
  const {
    workspaces,
    ready: workspaceListReady,
    unavailable: workspaceListUnavailable,
    isFetching: workspaceListRetrying,
    refetch: retryWorkspaceList,
  } = useWorkspaceList({
    enabled: !!user,
  });
  const wsCount = workspaces.length;
  const hasOnboarded = useHasOnboarded();

  // Bridge local daemon IPC status into the runtimes cache so this user's
  // own daemon flips to offline/online sub-second instead of waiting on the
  // server's 75s sweeper. Resolves wsId from the active tab so workspace
  // switches automatically rebind the subscription.
  const activeWorkspaceSlug = useTabStore((s) => s.activeWorkspaceSlug);
  const activeWsId = activeWorkspaceSlug
    ? workspaces.find((w) => w.slug === activeWorkspaceSlug)?.id
    : undefined;
  useDaemonIPCBridge(activeWsId);

  // Pre-workspace overlay routing for desktop. Mirrors the web layout
  // hard gate via overlays (desktop has no URL bar, so we open the
  // onboarding overlay instead of router.replace):
  //   onboarded + has workspace      → no overlay, dashboard
  //   un-onboarded (any wsCount):
  //     pending invites on email     → /invitations overlay
  //     no invites                   → /onboarding overlay
  //   onboarded + no workspace       → /workspaces/new overlay
  //
  // V3 invariant: `onboarded_at != null` is the only path into the
  // dashboard. CreateWorkspace does not mark onboarded; only Step 3's
  // CompleteOnboarding (and AcceptInvitation) flip the flag. A user who
  // somehow has a workspace but no onboarded mark must be sent back to
  // /onboarding — we also clear the active workspace so the dashboard
  // doesn't render under the overlay with stale workspace context.
  useEffect(() => {
    if (!user || !workspaceListReady) return undefined;
    const { overlay, open } = useWindowOverlayStore.getState();
    if (overlay) return undefined;
    if (hasOnboarded && wsCount > 0) return undefined;
    if (!hasOnboarded) {
      // Stale workspace context (if any) would leak X-Workspace-Slug
      // headers into onboarding-time API calls. Clear it before opening
      // the overlay.
      setCurrentWorkspace(null, null);
      // Look up pending invitations by email. Network blip is non-fatal —
      // fall through to onboarding so the user isn't stuck on a blank
      // window. The sidebar's pending-invitations dropdown will surface
      // missed invites later once they're onboarded.
      let cancelled = false;
      void api
        .listMyInvitations()
        .then((invites) => {
          if (cancelled) return;
          const { overlay: latestOverlay, open: latestOpen } =
            useWindowOverlayStore.getState();
          if (latestOverlay) return;
          if (invites.length > 0) {
            qc.setQueryData(workspaceKeys.myInvitations(), invites);
            latestOpen({ type: "invitations" });
          } else {
            latestOpen({ type: "onboarding" });
          }
        })
        .catch(() => {
          if (cancelled) return;
          const { overlay: latestOverlay, open: latestOpen } =
            useWindowOverlayStore.getState();
          if (latestOverlay) return;
          latestOpen({ type: "onboarding" });
        });
      return () => {
        cancelled = true;
      };
    }
    open({ type: "new-workspace" });
    return undefined;
  }, [user, workspaceListReady, wsCount, workspaces, hasOnboarded, qc]);


  // Validate persisted tab state against the current user's workspace list,
  // and pick an active workspace if none is set. Runs in useLayoutEffect
  // (synchronously after render, before paint) rather than the render
  // phase — the original render-phase pattern triggered React's
  // "Cannot update a component while rendering a different component"
  // warning because `switchWorkspace` is a Zustand setState that the
  // TabBar is subscribed to. useLayoutEffect flushes both renders before
  // the user sees anything, so there's no visible flicker.
  //
  // Gate on authoritative data: pending and initial errors expose no data,
  // while a failed background refetch retains the last successful list.
  useLayoutEffect(() => {
    if (!workspaceListReady) return;
    const validSlugs = new Set(workspaces.map((w) => w.slug));
    useTabStore.getState().validateWorkspaceSlugs(validSlugs);
    useWindowOverlayStore
      .getState()
      .validateSettingsWorkspace(validSlugs);
    const { activeWorkspaceSlug, switchWorkspace } = useTabStore.getState();
    if (!activeWorkspaceSlug && workspaces.length > 0) {
      switchWorkspace(workspaces[0].slug);
    }
  }, [workspaces, workspaceListReady]);

  // null = undecided (pre-login or list hasn't settled yet)
  // true  = session started with zero workspaces; next transition to >=1 triggers restart
  // false = session started with >=1 workspace, OR we've already restarted; skip
  const sessionStartedEmptyRef = useRef<boolean | null>(null);
  useEffect(() => {
    if (!user) {
      sessionStartedEmptyRef.current = null;
      return;
    }
    if (!workspaceListReady) return;
    if (sessionStartedEmptyRef.current === null) {
      sessionStartedEmptyRef.current = wsCount === 0;
      return;
    }
    if (sessionStartedEmptyRef.current && wsCount >= 1) {
      void window.daemonAPI.restart();
      sessionStartedEmptyRef.current = false;
    }
  }, [user, workspaceListReady, wsCount]);

  if (authStatus === "recovering") {
    return <DesktopAuthRecoveryPage />;
  }
  const daemonReadyForCurrentIdentity =
    !isElectronRenderer ||
    !user ||
    !runtimeConfig ||
    (daemonSyncState === "ready" && daemonSyncedKey === daemonIdentityKey);
  const daemonFailedForCurrentIdentity =
    !!user &&
    isElectronRenderer &&
    !!runtimeConfig &&
    daemonSyncState === "error" &&
    daemonSyncedKey === daemonIdentityKey;

  if (
    isLoading ||
    bootstrapping ||
    (user && !daemonReadyForCurrentIdentity && !daemonFailedForCurrentIdentity)
  ) {
    return (
      <div className="flex h-screen items-center justify-center">
        <PatchbayIcon className="size-6 animate-pulse" />
      </div>
    );
  }

  if (user && daemonFailedForCurrentIdentity) {
    return (
      <DesktopAuthRecoveryPage
        onRetry={() => setDaemonSyncRetry((attempt) => attempt + 1)}
        isRetrying={false}
        errorReason={daemonSyncError ?? undefined}
      />
    );
  }

  if (workspaceListUnavailable) {
    return (
      <DesktopAuthRecoveryPage
        isRetrying={workspaceListRetrying}
        onRetry={() => {
          void retryWorkspaceList();
        }}
      />
    );
  }

  return user ? <DesktopShell /> : <DesktopLoginPage />;
}

function BlockingRuntimeConfigError({ message }: { message: string }) {
  return (
    <div className="flex h-screen items-center justify-center bg-background p-8 text-foreground">
      <div className="max-w-xl rounded-lg border bg-card p-6 shadow-sm">
        <h1 className="text-title font-semibold">Desktop configuration error</h1>
        <p className="mt-3 text-body text-muted-foreground">
          Patchbay Desktop could not load <code>~/.patchbay/desktop.json</code>. Fix or remove the file and restart the app.
        </p>
        <pre className="mt-4 whitespace-pre-wrap rounded-md bg-muted p-3 text-caption text-muted-foreground">
          {message}
        </pre>
      </div>
    </div>
  );
}

// On logout, wipe desktop-only in-memory state and stop the daemon so that
// a subsequent login as a different user never inherits the previous user's
// tabs, overlay, or credentials. Zustand persist only writes to localStorage;
// useLogout clears the storage key, but the live stores stay populated until
// we explicitly reset them here.
async function handleDaemonLogout() {
  // The main-process clear-token operation owns one queue transaction: it
  // stops the current Desktop daemon, then removes its credentials. Keeping
  // this await before publishing `user=null` prevents a new login from
  // interleaving between stop and clear; failures leave the authenticated
  // session visible and retryable instead of silently abandoning the daemon.
  await window.daemonAPI.clearToken();

  // Report only after cleanup succeeds so issue windows never lose the session
  // while the old daemon is still running.
  window.desktopAPI.reportAuthSession?.(null);
  useTabStore.getState().reset();
  useWindowOverlayStore.getState().close();
  // Drop any post-onboarding welcome signal so user B logging in next
  // doesn't inherit user A's pending modal state.
  useWelcomeStore.getState().reset();
}

export default function App() {
  const { version, os } = window.desktopAPI.appInfo;
  const systemLocale = window.desktopAPI.systemLocale;
  const runtimeConfigResult = window.desktopAPI.runtimeConfig;
  // The fallback keeps renderer HMR safe while a main/preload rebuild is
  // restarting Electron; packaged builds always expose windowContext.
  const windowContext =
    window.desktopAPI.windowContext ?? { kind: "main" as const };
  const isBrowserRenderer = window.desktopAPI.host === "browser";
  useCmdWCloseTab();
  // Mounted at the App root for the same reason as Cmd+W: the chord has to
  // work in every renderer state, not only inside the tab shell.
  useOpenSettingsShortcut();

  // Flush a freeze/crash breadcrumb the main process parked from a previous
  // session. A true hang or process death can't report itself when it happens
  // (the renderer is blocked or gone), so the main process persists it and we
  // emit it here on the next boot. The in-thread, recoverable freeze tier is
  // handled separately by the shared watchdog in CoreProvider.
  useEffect(
    () =>
      flushFreezeBreadcrumb({
        getLastFreeze: () => window.desktopAPI.getLastFreeze(),
        ackFreeze: (ts) => window.desktopAPI.ackFreeze(ts),
        capture: captureEvent,
      }),
    [],
  );

  // Stable identity reference so downstream effects (WS reconnect) don't
  // tear down on every parent render.
  const identity = useMemo(
    () => ({ platform: "desktop", version, os }),
    [version, os],
  );
  // Locale resolution happens once at app boot. Switching language goes
  // through window.location.reload() to avoid hydration mismatch.
  const localeAdapter = useMemo(
    () => createDesktopLocaleAdapter(systemLocale),
    [systemLocale],
  );
  const locale = useMemo(() => pickLocale(localeAdapter), [localeAdapter]);
  const resources = useMemo(
    () => ({ [locale]: RESOURCES[locale] }),
    [locale],
  );

  // Keep <html lang> in sync with the resolved locale (index.html hardcodes
  // "en"). Drives the lang-scoped Japanese CJK font override and a11y.
  // useLayoutEffect (not useEffect) so lang is committed before the first
  // paint — otherwise Japanese users would see one frame of Kanji rendered
  // with the Chinese-first fallback stack before the override kicks in.
  useLayoutEffect(() => {
    document.documentElement.lang = HTML_LANG[locale];
  }, [locale]);

  // React to OS-level language changes detected by main on focus regain.
  // Only act when the user is following the system signal (no explicit
  // Settings choice) — otherwise their preference wins. Cross-device sync
  // for the explicit-choice case is handled inside CoreProvider.
  useEffect(() => {
    return window.desktopAPI.onSystemLocaleChanged((nextSystemLocale) => {
      if (localeAdapter.getUserChoice()) return;
      const next = pickLocale({
        ...localeAdapter,
        getSystemPreferences: () =>
          nextSystemLocale ? [nextSystemLocale] : [],
      });
      if (next === locale) return;
      localeAdapter.persist(next);
      window.location.reload();
    });
  }, [localeAdapter, locale]);

  return (
    <ThemeProvider>
      {runtimeConfigResult.ok ? (
        <CoreProvider
          apiBaseUrl={runtimeConfigResult.config.apiUrl}
          wsUrl={runtimeConfigResult.config.wsUrl}
          onLogout={
            windowContext.kind === "main" ? handleDaemonLogout : undefined
          }
          identity={identity}
          locale={locale}
          resources={resources}
          localeAdapter={localeAdapter}
        >
          <DesktopAuthSessionBridge />
          {windowContext.kind === "main" && <DiagnosticRouteReporter />}
          {windowContext.kind === "main" && !isBrowserRenderer && (
            <DesktopClientUsageReporter
              apiUrl={runtimeConfigResult.config.apiUrl}
            />
          )}
          {windowContext.kind === "issue" ? (
            <IssueWindowContent />
          ) : (
            <AppContent />
          )}
        </CoreProvider>
      ) : (
        <BlockingRuntimeConfigError message={runtimeConfigResult.error.message} />
      )}
      <Toaster />
      {windowContext.kind === "main" && <UpdateNotification />}
    </ThemeProvider>
  );
}
