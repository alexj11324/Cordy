import { beforeEach, describe, expect, it, vi } from "vitest";
import { handleDaemonLogout } from "./daemon-logout";

const cleanup = vi.hoisted(() => ({
  tabs: vi.fn(),
  overlay: vi.fn(),
  welcome: vi.fn(),
}));
vi.mock("../stores/tab-store", () => ({
  useTabStore: { getState: () => ({ reset: cleanup.tabs }) },
}));
vi.mock("../stores/window-overlay-store", () => ({
  useWindowOverlayStore: { getState: () => ({ close: cleanup.overlay }) },
}));
vi.mock("@patchbay/core/onboarding", () => ({
  useWelcomeStore: { getState: () => ({ reset: cleanup.welcome }) },
}));

const clearToken = vi.fn();
const stop = vi.fn();
const disableCloudMode = vi.fn();
const reportAuthSession = vi.fn();
beforeEach(() => {
  vi.resetAllMocks();
  Object.assign(window, {
    daemonAPI: { clearToken, stop },
    desktopAPI: { disableCloudMode, reportAuthSession },
  });
});

describe("desktop auth cleanup", () => {
  it("clears old account data but keeps the callback gate open before the first session", async () => {
    await handleDaemonLogout(undefined, { reason: "missing-session" });
    expect(reportAuthSession).toHaveBeenCalledWith(null);
    expect(cleanup.tabs).toHaveBeenCalledOnce();
    expect(cleanup.overlay).toHaveBeenCalledOnce();
    expect(cleanup.welcome).toHaveBeenCalledOnce();
    expect(clearToken).toHaveBeenCalledOnce();
    expect(stop).toHaveBeenCalledOnce();
    expect(disableCloudMode).not.toHaveBeenCalled();
  });

  it.each([undefined, { rearmAuth: false }])(
    "closes the callback gate for an explicit logout or rejected session (%j)",
    async (options) => {
      await handleDaemonLogout(undefined, options);
      expect(clearToken).toHaveBeenCalledOnce();
      expect(stop).toHaveBeenCalledOnce();
      expect(disableCloudMode).toHaveBeenCalledOnce();
    },
  );

  it("still closes the gate when daemon cleanup fails", async () => {
    clearToken.mockRejectedValue(new Error("not running"));
    stop.mockRejectedValue(new Error("not running"));
    await handleDaemonLogout();
    expect(disableCloudMode).toHaveBeenCalledOnce();
  });
});
