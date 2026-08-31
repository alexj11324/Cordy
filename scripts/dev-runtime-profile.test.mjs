import { describe, expect, it } from "vitest";

import {
  applyDevRuntimeProfile,
  assertDevRuntimeOverridesCompatible,
  parseDevRuntimeArgs,
  resolveDevRuntimeProfile,
} from "./dev-runtime-profile.mjs";

describe("development runtime profiles", () => {
  it("keeps local OAuth on one canonical browser origin", () => {
    expect(
      resolveDevRuntimeProfile("local", {
        PORT: "18123",
        FRONTEND_PORT: "13123",
      }),
    ).toEqual({
      mode: "local",
      apiUrl: "http://127.0.0.1:18123",
      wsUrl: "ws://127.0.0.1:18123/ws",
      appUrl: "http://localhost:13123",
      accountsUrl: "http://localhost:13123",
    });
  });

  it("uses the approved hosted accounts and API tuple", () => {
    const profile = resolveDevRuntimeProfile("hosted");

    expect(profile).toEqual({
      mode: "hosted",
      apiUrl: "https://api.aspectlylabs.com",
      wsUrl: "wss://api.aspectlylabs.com/ws",
      appUrl: "https://patchbay.aspectlylabs.com",
      accountsUrl: "https://accounts.aspectlylabs.com",
    });
  });

  it("sets the complete tuple instead of allowing stale partial VITE values", () => {
    const env = {
      VITE_API_URL: "http://127.0.0.1:18123",
      VITE_ACCOUNTS_URL: "http://localhost:13123",
    };

    applyDevRuntimeProfile(env, resolveDevRuntimeProfile("hosted"));

    expect(env).toMatchObject({
      PATCHBAY_DEV_MODE: "hosted",
      VITE_API_URL: "https://api.aspectlylabs.com",
      VITE_WS_URL: "wss://api.aspectlylabs.com/ws",
      VITE_APP_URL: "https://patchbay.aspectlylabs.com",
      VITE_ACCOUNTS_URL: "https://accounts.aspectlylabs.com",
      NEXT_PUBLIC_API_URL: "https://api.aspectlylabs.com",
      NEXT_PUBLIC_WS_URL: "wss://api.aspectlylabs.com/ws",
    });
  });

  it("removes the launcher-only mode flag before Electron", () => {
    expect(parseDevRuntimeArgs(["--hosted", "--inspect"])).toEqual({
      mode: "hosted",
      electronArgs: ["--inspect"],
    });
  });

  it("fails before launch when an inherited endpoint would create a mixed profile", () => {
    expect(() =>
      assertDevRuntimeOverridesCompatible("hosted", {
        VITE_API_URL: "http://127.0.0.1:18123",
      }),
    ).toThrow(/VITE_API_URL=.*conflicts with the hosted/);
  });
});
