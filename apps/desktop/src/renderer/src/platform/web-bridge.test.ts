// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  installWebDesktopBridge,
  isDesktopWebHost,
} from "./web-bridge";

const bridgeKeys = [
  "desktopAPI",
  "daemonAPI",
  "updater",
  "electron",
  "__PATCHBAY_VITE_DESKTOP_HOST__",
  "__PATCHBAY_VITE_DESKTOP_PREVIEW__",
] as const;

function clearWebBridge(): void {
  for (const key of bridgeKeys) {
    Reflect.deleteProperty(window, key);
  }
}

beforeEach(() => {
  clearWebBridge();
  window.history.replaceState(null, "", "/");
  vi.stubEnv("VITE_DESKTOP_PREVIEW", "false");
});

afterEach(() => {
  vi.unstubAllEnvs();
  clearWebBridge();
});

describe("Vite Desktop auth handoff", () => {
  it("delivers a validated one-time callback to the backend-enabled renderer", async () => {
    const code = `pbd_${"a".repeat(43)}`;
    const state = "b".repeat(43);
    window.history.replaceState(
      null,
      "",
      `/auth/callback?code=${code}&state=${state}`,
    );

    expect(installWebDesktopBridge()).toBe(true);
    expect(isDesktopWebHost()).toBe(true);
    expect(window.location.pathname).toBe("/");
    expect(window.location.search).toBe("");

    const callback = vi.fn();
    window.desktopAPI.onAuthHandoff(callback);
    await Promise.resolve();

    expect(callback).toHaveBeenCalledWith({ code, state });
  });

  it("does not deliver malformed or reusable credentials from the URL", async () => {
    window.history.replaceState(
      null,
      "",
      `/auth/callback?code=patchbay-long-session-token&state=${"b".repeat(43)}`,
    );

    expect(installWebDesktopBridge()).toBe(true);
    const callback = vi.fn();
    window.desktopAPI.onAuthHandoff(callback);
    await Promise.resolve();

    expect(callback).not.toHaveBeenCalled();
    expect(window.location.pathname).toBe("/");
    expect(window.location.search).toBe("");
  });

  it("keeps the no-backend fixture preview isolated from real handoffs", async () => {
    vi.stubEnv("VITE_DESKTOP_PREVIEW", "true");
    const code = `pbd_${"a".repeat(43)}`;
    const state = "b".repeat(43);
    window.history.replaceState(
      null,
      "",
      `/auth/callback?code=${code}&state=${state}`,
    );

    expect(installWebDesktopBridge()).toBe(true);
    const callback = vi.fn();
    window.desktopAPI.onAuthHandoff(callback);
    await Promise.resolve();

    expect(callback).not.toHaveBeenCalled();
  });
});

describe("Vite browser Desktop bridge", () => {
  it("lets shared views localize unsupported directory controls", async () => {
    expect(installWebDesktopBridge()).toBe(true);
    expect(window.desktopAPI.host).toBe("browser");

    await expect(window.desktopAPI.pickDirectory()).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
    await expect(
      window.desktopAPI.validateLocalDirectory("/tmp/local-directory"),
    ).resolves.toEqual({
      ok: false,
      reason: "unsupported",
    });
  });
});
