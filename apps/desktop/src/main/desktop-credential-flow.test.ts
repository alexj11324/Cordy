// @vitest-environment node
import { describe, expect, it } from "vitest";

import { commitDesktopCredentials } from "./desktop-credential-flow";

const baseInput = {
  previousToken: "pby-old",
  previousUserId: "user-a",
  previousServerUrl: "https://api.example.com",
  userId: "user-b",
  serverUrl: "https://api.example.com",
  incomingToken: "jwt-for-user-b",
  cachedTokenReusable: false,
};

function dependencies(events: string[], overrides: Partial<{
  inspect: { running: boolean; externallyManaged: boolean };
  stop: { success: boolean; error?: string; blocked?: boolean };
  minted: string;
  mintError: Error;
  restart: { success: boolean; error?: string; blocked?: boolean };
}> = {}) {
  return {
    inspectDaemon: async () => {
      events.push("inspect");
      return overrides.inspect ?? { running: true, externallyManaged: false };
    },
    stopDaemon: async () => {
      events.push("stop");
      return overrides.stop ?? { success: true };
    },
    resolveToken: async () => {
      events.push("mint");
      if (overrides.mintError) throw overrides.mintError;
      return overrides.minted ?? "pby-new";
    },
    writeCredentials: async (token: string) => {
      events.push(`write:${token}`);
    },
    restartDaemon: async () => {
      events.push("restart");
      return overrides.restart ?? { success: true };
    },
  };
}

describe("commitDesktopCredentials", () => {
  it("stops before minting and restarts only after an atomic write", async () => {
    const events: string[] = [];
    const result = await commitDesktopCredentials({
      ...baseInput,
      ...dependencies(events),
    });

    expect(events).toEqual(["inspect", "stop", "mint", "write:pby-new", "restart"]);
    expect(result).toEqual({
      credentialsChanged: true,
      daemonRestarted: true,
    });
  });

  it("leaves no old daemon running when minting fails", async () => {
    const events: string[] = [];
    await expect(
      commitDesktopCredentials({
        ...baseInput,
        ...dependencies(events, { mintError: new Error("mint unavailable") }),
      }),
    ).rejects.toThrow("mint unavailable");
    expect(events).toEqual(["inspect", "stop", "mint"]);
  });

  it("does not mint or write when the old daemon cannot be stopped", async () => {
    const events: string[] = [];
    await expect(
      commitDesktopCredentials({
        ...baseInput,
        ...dependencies(events, {
          stop: { success: false, error: "stop failed" },
        }),
      }),
    ).rejects.toThrow("stop failed");
    expect(events).toEqual(["inspect", "stop"]);
  });

  it("refuses an externally managed daemon before changing credentials", async () => {
    const events: string[] = [];
    await expect(
      commitDesktopCredentials({
        ...baseInput,
        ...dependencies(events, {
          inspect: { running: true, externallyManaged: true },
        }),
      }),
    ).rejects.toThrow(/externally managed/);
    expect(events).toEqual(["inspect"]);
  });

  it("propagates a failed restart instead of reporting successful sync", async () => {
    const events: string[] = [];
    await expect(
      commitDesktopCredentials({
        ...baseInput,
        ...dependencies(events, {
          restart: { success: false, error: "restart failed" },
        }),
      }),
    ).rejects.toThrow("restart failed");
    expect(events).toEqual(["inspect", "stop", "mint", "write:pby-new", "restart"]);
  });

  it("reuses an owner-matched cached PAT without a lifecycle stop/start", async () => {
    const events: string[] = [];
    const result = await commitDesktopCredentials({
      previousToken: "pby-cached",
      previousUserId: "user-a",
      previousServerUrl: "https://api.example.com",
      userId: "user-a",
      serverUrl: "https://api.example.com",
      incomingToken: "jwt-for-user-a",
      cachedTokenReusable: true,
      ...dependencies(events),
    });

    expect(events).toEqual(["write:pby-cached"]);
    expect(result).toEqual({
      credentialsChanged: false,
      daemonRestarted: false,
    });
  });
});
