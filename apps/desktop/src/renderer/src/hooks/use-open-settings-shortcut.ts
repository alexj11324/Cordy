import { useEffect } from "react";
import { useModalStore } from "@patchbay/core/modals";
import { paths } from "@patchbay/core/paths";
import { useTabStore } from "@/stores/tab-store";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";

/**
 * Open the first-class Settings page for the active workspace.
 *
 * Settings takes over the window without mutating the active tab. Closing it
 * therefore returns the user to the exact tab and history entry they left.
 *
 * No-ops when there is nothing to open into — logged out, zero workspaces, or
 * a pre-workspace overlay (onboarding, invite, create workspace) covering the
 * window.
 */
export function openSettingsPage(): void {
  const overlays = useWindowOverlayStore.getState();
  if (overlays.overlay?.type === "settings") return;
  if (overlays.overlay) return;
  const store = useTabStore.getState();
  const slug = store.activeWorkspaceSlug;
  if (!slug) return;
  // ModalRegistry portals live under document.body, outside the inert desktop
  // underlay. Close an active dialog before Settings takes over the window.
  useModalStore.getState().close();
  overlays.open({ type: "settings", path: paths.workspace(slug).settings() });
}

/**
 * Open Settings for the active workspace, the way Cmd/Ctrl+, does it.
 *
 * Open-or-activate (`openTab`) rather than in-tab navigation: a preferences
 * chord should never discard the page the user was working on, and pressing
 * it twice should return to the Settings tab that is already open instead of
 * stacking duplicates.
 *
 * No-ops when there is nothing to open into — logged out, zero workspaces, or
 * a pre-workspace overlay (onboarding, invite, create workspace) covering the
 * window, where a new tab would appear behind a full-window takeover.
 */
export function openSettingsTab(): void {
  if (useWindowOverlayStore.getState().overlay) return;
  const store = useTabStore.getState();
  const slug = store.activeWorkspaceSlug;
  if (!slug) return;
  // Empty seed title: the tab bar derives the real one from the URL, same as
  // every other programmatic openTab caller.
  store.openTab(paths.workspace(slug).settings(), "", { activate: true });
}

/**
 * Cmd/Ctrl+, opens Settings — matched in the main process (see
 * main/keyboard-shortcuts.ts) so it fires from any window and any focus
 * context, then delivered here.
 *
 * Only the tabbed main window subscribes: main routes the chord to that
 * window regardless of which one had focus, and a second subscriber in an
 * issue window would drain the request into a renderer that has no tabs.
 */
export function useOpenSettingsShortcut(): void {
  useEffect(() => {
    if (window.desktopAPI.windowContext?.kind === "issue") return undefined;
    // Optional call keeps renderer HMR safe while an old preload is still
    // attached to a refreshed React tree, same as reportAuthSession in App.
    return window.desktopAPI.onOpenSettings?.(openSettingsPage);
  }, []);
}
