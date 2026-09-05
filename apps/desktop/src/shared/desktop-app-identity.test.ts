// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  desktopChannelFromArgv,
  resolveDesktopAppIdentity,
  resolveDesktopChannel,
} from "./desktop-app-identity";

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
});

describe("desktop app identity", () => {
  it("keeps packaged production on the default Electron userData path", () => {
    expect(resolveDesktopAppIdentity({ isDev: false })).toEqual({
      channel: "production",
      name: "Patchbay",
      userDataDirName: "Patchbay",
      appUserModelId: "ai.patchbay.desktop",
      bundleIdPrefix: "ai.patchbay.desktop",
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
    expect(canary.name).toBe("Patchbay Canary");
    expect(staging.name).toBe("Patchbay Staging");
    expect(new Set([canary.userDataDirName, staging.userDataDirName, production.userDataDirName]).size).toBe(3);
    expect(canary.isolateUserData).toBe(true);
    expect(staging.isolateUserData).toBe(true);
    expect(canary.appUserModelId).not.toBe(staging.appUserModelId);
    expect(staging.appUserModelId).not.toBe(production.appUserModelId);
  });

  it("keeps worktree suffixes inside the same channel", () => {
    expect(
      resolveDesktopAppIdentity({
        isDev: true,
        mode: "staging",
        suffix: "feature-12",
      }).name,
    ).toBe("Patchbay Staging feature-12");
    expect(
      resolveDesktopAppIdentity({ isDev: true, suffix: "feature-12" }).name,
    ).toBe("Patchbay Canary feature-12");
  });
});
