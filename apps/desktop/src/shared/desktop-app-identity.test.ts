// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  checkoutCallbackProtocol,
  desktopChannelFromArgv,
  identityHashForPath,
  isDesktopCallbackProtocolForChannel,
  protocolClientLaunchArgs,
  resolveDesktopAppIdentity,
  resolveDesktopChannel,
} from "./desktop-app-identity";
import { callbackProtocolForPath, identityHashForPath as scriptIdentityHashForPath } from "../../scripts/worktree-dev-env.mjs";

describe("desktop channel", () => {
  it("treats packaged builds as production regardless of vite mode", () => {
    expect(resolveDesktopChannel({ isDev: false, mode: "staging" })).toBe(
      "production",
    );
    expect(resolveDesktopChannel({ isDev: false, mode: "development" })).toBe(
      "production",
    );
  });

  it("splits electron-vite staging from local Canary", () => {
    expect(resolveDesktopChannel({ isDev: true, mode: "staging" })).toBe(
      "staging",
    );
    expect(resolveDesktopChannel({ isDev: true, mode: "development" })).toBe(
      "development",
    );
    expect(resolveDesktopChannel({ isDev: true })).toBe("development");
  });

  it("reads --mode staging from the desktop launcher argv", () => {
    expect(desktopChannelFromArgv(["--mode", "staging"])).toBe("staging");
    expect(desktopChannelFromArgv(["dev"])).toBe("development");
    expect(desktopChannelFromArgv(["--mode", "development"])).toBe(
      "development",
    );
  });

  it("keeps an OS-launched staging protocol handler on the staging channel", () => {
    expect(
      resolveDesktopChannel({
        isDev: true,
        mode: "development",
        argv: ["electron", ".", "--mode", "staging"],
      }),
    ).toBe("staging");
    expect(
      resolveDesktopChannel({
        isDev: false,
        argv: ["--mode", "staging"],
      }),
    ).toBe("production");
  });
});

describe("desktop app identity", () => {
  it("keeps packaged production on the pre-rebrand userData path", () => {
    expect(resolveDesktopAppIdentity({ isDev: false })).toEqual({
      channel: "production",
      name: "Orvilo",
      userDataDirName: "Orvilo",
      appUserModelId: "ai.orvilo.desktop",
      bundleIdPrefix: "ai.orvilo.desktop",
      callbackProtocolPrefix: "orvilo",
      isolateUserData: false,
    });
  });

  it("isolates local Canary from staging and production", () => {
    const canary = resolveDesktopAppIdentity({ isDev: true });
    const staging = resolveDesktopAppIdentity({
      isDev: true,
      mode: "staging",
    });
    const production = resolveDesktopAppIdentity({ isDev: false });
    expect(canary.name).toBe("Orvilo Canary");
    expect(staging.name).toBe("Orvilo Staging");
    expect(canary.userDataDirName).toBe("Orvilo Canary");
    expect(staging.userDataDirName).toBe("Orvilo Staging");
    expect(
      new Set([
        canary.userDataDirName,
        staging.userDataDirName,
        production.userDataDirName,
      ]).size,
    ).toBe(3);
    expect(canary.isolateUserData).toBe(true);
    expect(staging.isolateUserData).toBe(true);
    expect(canary.appUserModelId).not.toBe(staging.appUserModelId);
    expect(staging.appUserModelId).not.toBe(production.appUserModelId);
    expect(canary.callbackProtocolPrefix).toBe("orvilo-canary");
    expect(staging.callbackProtocolPrefix).toBe("orvilo-staging");
    expect(production.callbackProtocolPrefix).toBe("orvilo");
    expect(
      new Set([
        canary.callbackProtocolPrefix,
        staging.callbackProtocolPrefix,
        production.callbackProtocolPrefix,
      ]).size,
    ).toBe(3);
  });

  it("keeps worktree suffixes inside the same channel", () => {
    const staging = resolveDesktopAppIdentity({
      isDev: true,
      mode: "staging",
      suffix: "feature-12",
    });
    expect(staging.name).toBe("Orvilo Staging feature-12");
    expect(staging.userDataDirName).toBe("Orvilo Staging feature-12");
    expect(staging.callbackProtocolPrefix).toBe("orvilo-staging");
    const canary = resolveDesktopAppIdentity({
      isDev: true,
      suffix: "feature-12",
    });
    expect(canary.name).toBe("Orvilo Canary feature-12");
    expect(canary.userDataDirName).toBe("Orvilo Canary feature-12");
    expect(canary.callbackProtocolPrefix).toBe("orvilo-canary");
  });

  it("rejects a callback scheme that belongs to another channel", () => {
    expect(isDesktopCallbackProtocolForChannel("orvilo", "production")).toBe(
      true,
    );
    expect(
      isDesktopCallbackProtocolForChannel(
        "orvilo-staging-5718c47b86bf9ece",
        "staging",
      ),
    ).toBe(true);
    expect(
      isDesktopCallbackProtocolForChannel(
        "orvilo-canary-5718c47b86bf9ece",
        "development",
      ),
    ).toBe(true);
    expect(isDesktopCallbackProtocolForChannel("orvilo", "staging")).toBe(
      false,
    );
    expect(
      isDesktopCallbackProtocolForChannel(
        "orvilo-canary-5718c47b86bf9ece",
        "staging",
      ),
    ).toBe(false);
    expect(
      isDesktopCallbackProtocolForChannel(
        "orvilo-staging-5718c47b86bf9ece",
        "development",
      ),
    ).toBe(false);
  });

  it("derives the same checkout callback scheme the pre-boot launcher registers", () => {
    const appPath = "/worktrees/first/apps/desktop";
    expect(identityHashForPath(appPath)).toBe(scriptIdentityHashForPath(appPath));
    expect(checkoutCallbackProtocol("staging", appPath)).toBe(
      callbackProtocolForPath(appPath, "staging"),
    );
    expect(checkoutCallbackProtocol("development", appPath)).toBe(
      callbackProtocolForPath(appPath),
    );
    expect(checkoutCallbackProtocol("production", appPath)).toBe("orvilo");
    expect(checkoutCallbackProtocol("staging", appPath)).not.toBe(
      checkoutCallbackProtocol("development", appPath),
    );
  });

  it("relaunches a Windows staging protocol handler with --mode staging", () => {
    expect(protocolClientLaunchArgs("/app", "staging")).toEqual([
      "/app",
      "--mode",
      "staging",
    ]);
    expect(protocolClientLaunchArgs("/app", "development")).toEqual(["/app"]);
    expect(protocolClientLaunchArgs("/app", "production")).toEqual(["/app"]);
  });
});
