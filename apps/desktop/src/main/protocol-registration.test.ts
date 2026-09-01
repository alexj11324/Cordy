import { describe, expect, it, vi } from "vitest";

import {
  LEGACY_PROTOCOL,
  findDesktopProtocolUrl,
  readDesktopAppSuffix,
  readDesktopCallbackProtocol,
  registerDesktopProtocolClients,
} from "./protocol-registration";

function createApp() {
  return {
    getAppPath: vi.fn(() => "/worktrees/patchbay/apps/desktop"),
    setAsDefaultProtocolClient: vi.fn(() => true),
  };
}

describe("registerDesktopProtocolClients", () => {
  it("finds the isolated callback in Windows and Linux launch arguments", () => {
    expect(
      findDesktopProtocolUrl(
        [
          "/path/to/electron",
          "patchbay-canary-login-fix-123://auth/callback?code=one-time",
        ],
        "patchbay-canary-login-fix-123",
      ),
    ).toBe("patchbay-canary-login-fix-123://auth/callback?code=one-time");
  });

  it("recovers the registered callback protocol after a cold start", () => {
    expect(
      readDesktopCallbackProtocol([
        "/worktrees/patchbay/node_modules/electron/electron",
        "--desktop-auth-callback-protocol=patchbay-canary-login-fix-123",
        "patchbay-canary-login-fix-123://auth/callback?code=one-time",
      ]),
    ).toBe("patchbay-canary-login-fix-123");
    expect(
      readDesktopCallbackProtocol([
        "--desktop-auth-callback-protocol=evil-app",
      ]),
    ).toBeUndefined();
  });

  it("recovers only a safe app suffix after a cold start", () => {
    expect(
      readDesktopAppSuffix(["--desktop-app-suffix=login-fix-123"]),
    ).toBe("login-fix-123");
    expect(readDesktopAppSuffix(["--desktop-app-suffix=../shared"])).toBeUndefined();
  });

  it("does not accept a different Canary worktree protocol", () => {
    expect(
      findDesktopProtocolUrl(
        ["patchbay-canary-attacker://auth/callback?code=one-time"],
        "patchbay-canary-login-fix-123",
      ),
    ).toBeUndefined();
  });

  it("registers only the isolated callback scheme for macOS development", () => {
    const app = createApp();

    registerDesktopProtocolClients(app, {
      isDefaultApp: true,
      platform: "darwin",
      execPath: "/worktrees/patchbay/node_modules/electron/Electron",
      authCallbackProtocol: "patchbay-canary-login-fix-123",
      desktopAppSuffix: "login-fix-123",
    });

    expect(app.setAsDefaultProtocolClient.mock.calls).toEqual([
      [
        "patchbay-canary-login-fix-123",
        "/worktrees/patchbay/node_modules/electron/Electron",
        [
          "/worktrees/patchbay/apps/desktop",
          "--desktop-auth-callback-protocol=patchbay-canary-login-fix-123",
          "--desktop-app-suffix=login-fix-123",
        ],
      ],
    ]);
  });

  it("registers production schemes for the packaged app", () => {
    const app = createApp();

    registerDesktopProtocolClients(app, {
      isDefaultApp: false,
      platform: "darwin",
      execPath: "/Applications/Patchbay.app/Contents/MacOS/Patchbay",
      authCallbackProtocol: "patchbay",
    });

    expect(app.setAsDefaultProtocolClient.mock.calls).toEqual([
      ["patchbay"],
      [LEGACY_PROTOCOL],
    ]);
  });

  it("keeps executable arguments and an isolated scheme for development off macOS", () => {
    const app = createApp();

    registerDesktopProtocolClients(app, {
      isDefaultApp: true,
      platform: "linux",
      execPath: "/worktrees/patchbay/node_modules/electron/electron",
      authCallbackProtocol: "patchbay-canary-linux-456",
    });

    expect(app.setAsDefaultProtocolClient.mock.calls).toEqual([
      [
        "patchbay-canary-linux-456",
        "/worktrees/patchbay/node_modules/electron/electron",
        [
          "/worktrees/patchbay/apps/desktop",
          "--desktop-auth-callback-protocol=patchbay-canary-linux-456",
        ],
      ],
    ]);
  });
});
