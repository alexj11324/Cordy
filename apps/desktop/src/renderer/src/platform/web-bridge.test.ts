// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installWebDesktopBridge } from "./web-bridge";

const bridgeKeys = [
  "desktopAPI",
  "daemonAPI",
  "updater",
  "electron",
] as const;

function clearWebBridge(): void {
  for (const key of bridgeKeys) {
    Reflect.deleteProperty(window, key);
  }
}

beforeEach(() => {
  clearWebBridge();
  sessionStorage.clear();
  window.history.replaceState(null, "", "/");
});

afterEach(() => {
  vi.unstubAllEnvs();
  sessionStorage.clear();
  clearWebBridge();
});

describe("Vite browser Desktop bridge", () => {
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

  it("keeps hosted Accounts when the local product API is on localhost", () => {
    vi.stubEnv("VITE_API_URL", "http://localhost:8080");

    expect(installWebDesktopBridge()).toBe(true);
    expect(window.desktopAPI.runtimeConfig).toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "http://localhost:8080",
        wsUrl: "ws://localhost:8080/ws",
        appUrl: "http://localhost:3000",
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

  it("does not register an HTTP auth callback transport", () => {
    vi.stubEnv("VITE_API_URL", "https://api.aspectlylabs.com");
    window.history.replaceState(
      null,
      "",
      `/auth/callback?code=pbd_${"a".repeat(43)}&state=${"b".repeat(43)}`,
    );

    expect(installWebDesktopBridge()).toBe(true);
    const callback = vi.fn();
    window.desktopAPI.onAuthHandoff(callback);

    expect(callback).not.toHaveBeenCalled();
    expect(window.location.pathname).toBe("/auth/callback");
    expect(window.location.search).toContain("code=");
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
