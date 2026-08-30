// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  installWebDesktopBridge,
  isDesktopWebHost,
  isDesktopWebPreview,
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
});

afterEach(() => {
  vi.unstubAllEnvs();
  clearWebBridge();
});

describe("Vite Desktop auth handoff", () => {
  it("keeps API, app, and accounts origins distinct when configured", () => {
    vi.stubEnv("VITE_API_URL", "https://api.aspectlylabs.com");
    vi.stubEnv("VITE_APP_URL", "https://patchbay.aspectlylabs.com");
    vi.stubEnv("VITE_ACCOUNTS_URL", "https://accounts.aspectlylabs.com");

    expect(installWebDesktopBridge()).toBe(true);
    expect(window.desktopAPI.runtimeConfig).toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "https://api.aspectlylabs.com",
        wsUrl: "wss://api.aspectlylabs.com/ws",
        appUrl: "https://patchbay.aspectlylabs.com",
        accountsUrl: "https://accounts.aspectlylabs.com",
      },
    });
  });

  it("keeps the hosted broker when only the backend URL is configured", () => {
    vi.stubEnv("VITE_API_URL", "https://api.aspectlylabs.com");

    expect(installWebDesktopBridge()).toBe(true);
    expect(window.desktopAPI.runtimeConfig).toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "https://api.aspectlylabs.com",
        wsUrl: "wss://api.aspectlylabs.com/ws",
        appUrl: "https://patchbay.aspectlylabs.com",
        accountsUrl: "https://accounts.aspectlylabs.com",
      },
    });
  });

  it("delivers a validated one-time callback to the backend-enabled renderer", async () => {
    vi.stubEnv("VITE_API_URL", "https://api.aspectlylabs.com");
    const code = `pbd_${"a".repeat(43)}`;
    const state = "b".repeat(43);
    window.history.replaceState(
      null,
      "",
      `/auth/callback?code=${code}&state=${state}`,
    );

    expect(installWebDesktopBridge()).toBe(true);
    expect(isDesktopWebHost()).toBe(true);
    expect(isDesktopWebPreview()).toBe(false);
    expect(window.location.pathname).toBe("/");
    expect(window.location.search).toBe("");

    const callback = vi.fn().mockResolvedValue(true);
    window.desktopAPI.onAuthHandoff(callback);
    await Promise.resolve();

    expect(callback).toHaveBeenCalledWith({ code, state });
  });

  it("does not deliver malformed or reusable credentials from the URL", async () => {
    vi.stubEnv("VITE_API_URL", "https://api.aspectlylabs.com");
    window.history.replaceState(
      null,
      "",
      `/auth/callback?code=patchbay-long-session-token&state=${"b".repeat(43)}`,
    );

    expect(installWebDesktopBridge()).toBe(true);
    const callback = vi.fn().mockResolvedValue(true);
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
    const callback = vi.fn().mockResolvedValue(true);
    window.desktopAPI.onAuthHandoff(callback);
    await Promise.resolve();

    expect(callback).not.toHaveBeenCalled();
  });

  it("retains a browser handoff until redemption is acknowledged", async () => {
    vi.stubEnv("VITE_API_URL", "https://api.aspectlylabs.com");
    const code = `pbd_${"a".repeat(43)}`;
    const state = "b".repeat(43);
    window.history.replaceState(
      null,
      "",
      `/auth/callback?code=${code}&state=${state}`,
    );

    expect(installWebDesktopBridge()).toBe(true);
    const callback = vi
      .fn()
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true);
    const unsubscribe = window.desktopAPI.onAuthHandoff(callback);
    await Promise.resolve();
    await Promise.resolve();

    expect(callback).toHaveBeenCalledOnce();

    window.dispatchEvent(new Event("online"));
    await Promise.resolve();
    await Promise.resolve();

    expect(callback).toHaveBeenCalledTimes(2);

    window.dispatchEvent(new Event("online"));
    await Promise.resolve();
    await Promise.resolve();
    expect(callback).toHaveBeenCalledTimes(2);
    unsubscribe();
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
