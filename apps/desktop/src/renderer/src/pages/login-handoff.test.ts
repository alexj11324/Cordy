// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@patchbay/core/api";
import {
  clearDesktopHandoffVerifier,
  completeDesktopHandoff,
  createDesktopGoogleLoginUrl,
  readDesktopHandoffVerifier,
} from "./login-handoff";

describe("desktop login handoff", () => {
  beforeEach(() => {
    sessionStorage.clear();
    localStorage.clear();
    vi.stubGlobal("crypto", {
      getRandomValues: (bytes: Uint8Array) => {
        bytes.fill(7);
        return bytes;
      },
      subtle: {
        digest: vi.fn(async () => new Uint8Array(32).buffer),
      },
    });
  });

  it("stores a renderer-bound verifier and carries only challenge/state to web login", async () => {
    const url = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const parsed = new URL(url);

    expect(parsed.origin).toBe("https://accounts.aspectlylabs.com");
    expect(parsed.pathname).toBe("/oauth/google");
    expect(parsed.searchParams.get("platform")).toBe("desktop");
    expect(parsed.searchParams.get("code_challenge")).toHaveLength(43);
    const state = parsed.searchParams.get("state");
    expect(state).toHaveLength(43);
    expect(parsed.searchParams.get("token")).toBeNull();
    expect(readDesktopHandoffVerifier(state ?? "")).toHaveLength(43);
  });

  it("does not clear the pending verifier for an unsolicited state", async () => {
    await createDesktopGoogleLoginUrl("https://accounts.aspectlylabs.com");

    expect(readDesktopHandoffVerifier("wrong-state")).toBeNull();
    const raw = localStorage.getItem("patchbay_desktop_login_handoff");
    expect(raw).not.toBeNull();
  });

  it("keeps the verifier after the renderer session is recreated", async () => {
    const url = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const state = new URL(url).searchParams.get("state") ?? "";

    sessionStorage.clear();

    expect(readDesktopHandoffVerifier(state)).toHaveLength(43);
  });

  it("rejects an expired verifier", async () => {
    vi.useFakeTimers();
    try {
      const url = await createDesktopGoogleLoginUrl(
        "https://accounts.aspectlylabs.com",
      );
      const state = new URL(url).searchParams.get("state") ?? "";

      vi.advanceTimersByTime(10 * 60 * 1000);

      expect(readDesktopHandoffVerifier(state)).toBeNull();
      expect(localStorage.getItem("patchbay_desktop_login_handoff")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("clears the verifier only for the matching completed handoff", async () => {
    const url = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const state = new URL(url).searchParams.get("state") ?? "";

    clearDesktopHandoffVerifier(state);

    expect(readDesktopHandoffVerifier(state)).toBeNull();
  });

  it("retains independent verifiers when multiple browser logins are pending", async () => {
    let seed = 7;
    vi.stubGlobal("crypto", {
      getRandomValues: (bytes: Uint8Array) => {
        bytes.fill(seed++);
        return bytes;
      },
      subtle: {
        digest: vi.fn(async () => new Uint8Array(32).buffer),
      },
    });

    const firstUrl = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const secondUrl = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const firstState = new URL(firstUrl).searchParams.get("state") ?? "";
    const secondState = new URL(secondUrl).searchParams.get("state") ?? "";

    expect(firstState).not.toBe(secondState);
    expect(readDesktopHandoffVerifier(firstState)).toHaveLength(43);
    expect(readDesktopHandoffVerifier(secondState)).toHaveLength(43);

    clearDesktopHandoffVerifier(firstState);

    expect(readDesktopHandoffVerifier(firstState)).toBeNull();
    expect(readDesktopHandoffVerifier(secondState)).toHaveLength(43);
  });

  it("recovers from the persisted token after redeem succeeds but user hydration fails", async () => {
    const url = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const state = new URL(url).searchParams.get("state") ?? "";
    const redeem = vi.fn().mockResolvedValue({ token: "session-token" });
    const login = vi.fn().mockRejectedValue(new TypeError("temporarily offline"));
    const recoverPersistedToken = vi.fn();

    await expect(
      completeDesktopHandoff("pbd_code", state, {
        redeem,
        login,
        recoverPersistedToken,
      }),
    ).resolves.toEqual({ acknowledged: true, authenticated: false });

    expect(login).toHaveBeenCalledWith("session-token");
    expect(recoverPersistedToken).toHaveBeenCalledOnce();
    expect(readDesktopHandoffVerifier(state)).toBeNull();
  });

  it("acknowledges a code only after redemption and authentication succeed", async () => {
    const url = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const state = new URL(url).searchParams.get("state") ?? "";

    await expect(
      completeDesktopHandoff("pbd_code", state, {
        redeem: vi.fn().mockResolvedValue({ token: "session-token" }),
        login: vi.fn().mockResolvedValue(undefined),
        recoverPersistedToken: vi.fn(),
      }),
    ).resolves.toEqual({ acknowledged: true, authenticated: true });

    expect(readDesktopHandoffVerifier(state)).toBeNull();
  });

  it("keeps the verifier when the one-time code was not redeemed", async () => {
    const url = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const state = new URL(url).searchParams.get("state") ?? "";
    const recoverPersistedToken = vi.fn();

    await expect(
      completeDesktopHandoff("invalid-code", state, {
        redeem: vi.fn().mockRejectedValue(new Error("invalid handoff")),
        login: vi.fn(),
        recoverPersistedToken,
      }),
    ).rejects.toThrow("invalid handoff");

    expect(recoverPersistedToken).not.toHaveBeenCalled();
    expect(readDesktopHandoffVerifier(state)).toHaveLength(43);
  });

  it("retires the verifier after a terminal redeem rejection", async () => {
    const url = await createDesktopGoogleLoginUrl(
      "https://accounts.aspectlylabs.com",
    );
    const state = new URL(url).searchParams.get("state") ?? "";

    await expect(
      completeDesktopHandoff("invalid-code", state, {
        redeem: vi.fn().mockRejectedValue(
          new ApiError("invalid desktop handoff", 401, "Unauthorized"),
        ),
        login: vi.fn(),
        recoverPersistedToken: vi.fn(),
      }),
    ).resolves.toEqual({ acknowledged: true, authenticated: false });

    expect(readDesktopHandoffVerifier(state)).toBeNull();
  });
});
