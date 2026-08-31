import { describe, expect, it, vi } from "vitest";

import {
  LEGACY_PROTOCOL,
  findDesktopProtocolUrl,
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
    });

    expect(app.setAsDefaultProtocolClient.mock.calls).toEqual([
      [
        "patchbay-canary-login-fix-123",
        "/worktrees/patchbay/node_modules/electron/Electron",
        ["/worktrees/patchbay/apps/desktop"],
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
        ["/worktrees/patchbay/apps/desktop"],
      ],
    ]);
  });
});
