import type { AuthLogoutHandler } from "@patchbay/core/auth";
import { useWelcomeStore } from "@patchbay/core/onboarding";
import { useTabStore } from "../stores/tab-store";
import { useWindowOverlayStore } from "../stores/window-overlay-store";

// On logout, wipe desktop-only in-memory state and stop the daemon so that
// a subsequent login as a different user never inherits the previous user's
// tabs, overlay, or credentials. Zustand persist only writes to localStorage;
// useLogout clears the storage key, but the live stores stay populated until
// we explicitly reset them here.
export const handleDaemonLogout: AuthLogoutHandler = async (_serverLogout, options) => {
  // Report synchronously before async daemon cleanup so a rapidly closed main
  // window cannot leave authenticated issue renderers behind.
  window.desktopAPI.reportAuthSession?.(null);
  useTabStore.getState().reset();
  useWindowOverlayStore.getState().close();
  // Drop any post-onboarding welcome signal so user B logging in next
  // doesn't inherit user A's pending modal state.
  useWelcomeStore.getState().reset();
  try {
    await window.daemonAPI.clearToken();
  } catch {
    // Best-effort — clearing is followed by stop which also hardens state.
  }
  try {
    await window.daemonAPI.stop();
  } catch {
    // Daemon may already be stopped.
  }
  // No stored session is normal while the browser completes the first login.
  // Keep the cloud callback listener mounted after clearing stale local state.
  if (options?.reason === "missing-session") return;
  try {
    await window.desktopAPI.disableCloudMode();
  } catch {
    // Main keeps the cloud mode gate closed even when teardown is best effort.
  }
};
