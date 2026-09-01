// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonAutoStartResult, DaemonStatus } from "../../../shared/daemon-types";

import {
  syncDaemonOnLogin,
  type DaemonLoginSyncAPI,
  waitForDaemonRunning,
} from "./daemon-login-sync";

const calls: string[] = [];

function makeApi(overrides: Partial<DaemonLoginSyncAPI> = {}): DaemonLoginSyncAPI {
  const status: DaemonStatus = { state: "running", profile: "dev" };
  return {
    setTargetApiUrl: vi.fn(async () => {
      calls.push("setTargetApiUrl");
    }),
    syncToken: vi.fn(async () => {
      calls.push("syncToken");
    }),
    autoStart: vi.fn(async (): Promise<DaemonAutoStartResult> => {
      calls.push("autoStart");
      return { success: true, state: "running", profile: "dev" };
    }),
    getStatus: vi.fn(async () => status),
    onStatusChange: vi.fn(() => () => undefined),
    ...overrides,
  };
}

beforeEach(() => {
  calls.length = 0;
  vi.clearAllMocks();
});

describe("syncDaemonOnLogin", () => {
  // Regression: syncToken used to race a separate effect's setTargetApiUrl.
  // Arriving first meant main had no resolved profile and wrote the token to
  // the user's default CLI profile. #6399.
  it("pushes the target URL before syncing the token", async () => {
    const api = makeApi();
    await syncDaemonOnLogin(api, "https://api.example.com", "tok", "user-1");

    expect(calls).toEqual(["setTargetApiUrl", "syncToken", "autoStart"]);
    expect(api.setTargetApiUrl).toHaveBeenCalledWith("https://api.example.com");
    expect(api.syncToken).toHaveBeenCalledWith("tok", "user-1");
  });

  it("awaits the target URL rather than firing it off", async () => {
    let released: (() => void) | undefined;
    const api = makeApi({
      setTargetApiUrl: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            released = () => {
              calls.push("setTargetApiUrl");
              resolve();
            };
          }),
      ),
    });

    const pending = syncDaemonOnLogin(api, "https://api.example.com", "t", "u");
    await Promise.resolve();
    expect(api.syncToken).not.toHaveBeenCalled();

    released?.();
    await pending;
    expect(calls).toEqual(["setTargetApiUrl", "syncToken", "autoStart"]);
  });

  it("does not start the daemon when the token sync fails", async () => {
    const api = makeApi({
      syncToken: vi.fn(async () => {
        throw new Error("daemon profile is not resolved yet");
      }),
    });

    await expect(
      syncDaemonOnLogin(api, "https://api.example.com", "t", "u"),
    ).rejects.toThrow(/not resolved/);
    expect(api.autoStart).not.toHaveBeenCalled();
  });

  it("does not mount a complete session when auto-start reports a failure", async () => {
    const api = makeApi({
      autoStart: vi.fn(async () => ({
        success: false as const,
        state: "cli_not_found" as const,
        reason: "cli_not_found" as const,
        error: "source-matched CLI is unavailable",
      })),
    });

    await expect(
      syncDaemonOnLogin(api, "https://api.example.com", "t", "u"),
    ).rejects.toThrow(/source-matched CLI/);
    expect(api.getStatus).not.toHaveBeenCalled();
  });

  it("waits for a starting daemon to emit running before resolving", async () => {
    let listener: ((status: DaemonStatus) => void) | undefined;
    const api = makeApi({
      autoStart: vi.fn(async () => ({
        success: true as const,
        state: "starting" as const,
      })),
      getStatus: vi
        .fn()
        .mockResolvedValueOnce({ state: "starting" as const })
        .mockResolvedValueOnce({ state: "starting" as const })
        .mockResolvedValueOnce({ state: "running" as const, profile: "dev" }),
      onStatusChange: vi.fn((callback: (status: DaemonStatus) => void) => {
        listener = callback;
        return () => {
          listener = undefined;
        };
      }),
    });

    let settled = false;
    const pending = syncDaemonOnLogin(api, "https://api.example.com", "t", "u").then(
      () => {
        settled = true;
      },
    );
    await vi.waitFor(() => expect(api.getStatus).toHaveBeenCalled());
    expect(settled).toBe(false);
    listener?.({ state: "running", profile: "dev" });
    await pending;
    expect(settled).toBe(true);
    expect(api.onStatusChange).toHaveBeenCalledOnce();
  });

  it("treats a stale profile event as a wake-up, not readiness proof", async () => {
    let listener: (() => void) | undefined;
    const api = {
      getStatus: vi
        .fn()
        .mockResolvedValueOnce({ state: "starting", profile: "new" })
        .mockResolvedValueOnce({ state: "running", profile: "new" }),
      onStatusChange: vi.fn((callback: (status: DaemonStatus) => void) => {
        listener = () => callback({ state: "running", profile: "old" });
        return () => {
          listener = undefined;
        };
      }),
    };

    let settled = false;
    const pending = waitForDaemonRunning(api, {
      expectedProfile: "new",
      timeoutMs: 100,
      pollMs: 50,
    }).then(() => {
      settled = true;
    });
    await vi.waitFor(() => expect(api.getStatus).toHaveBeenCalledOnce());
    expect(settled).toBe(false);
    listener?.();
    await pending;
    expect(settled).toBe(true);
  });

  it("fails with an actionable timeout and unsubscribes", async () => {
    const unsubscribe = vi.fn();
    const api = {
      getStatus: vi.fn(async () => ({ state: "starting" as const })),
      onStatusChange: vi.fn(() => unsubscribe),
    };

    await expect(
      waitForDaemonRunning(api, { timeoutMs: 5, pollMs: 1 }),
    ).rejects.toThrow(/did not become ready within/);
    expect(unsubscribe).toHaveBeenCalledOnce();
  });
});
