import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@patchbay/core/api";
import {
  completeDesktopHandoff,
  createDesktopGoogleLoginUrl,
} from "./login-handoff";

const PENDING_HANDOFF_KEY = "patchbay_desktop_login_handoff";

function pendingHandoff(): {
  state: string;
  verifier: string;
  expiresAt: number;
} {
  const raw = localStorage.getItem(PENDING_HANDOFF_KEY);
  if (!raw) throw new Error("desktop handoff was not persisted");
  const value: unknown = JSON.parse(raw);
  if (!Array.isArray(value) || value.length !== 1) {
    throw new Error("unexpected pending handoff shape");
  }
  return value[0] as {
    state: string;
    verifier: string;
    expiresAt: number;
  };
}

describe("desktop auth handoff", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("registers a PKCE binding before building the browser URL", async () => {
    const initiate = vi.fn().mockResolvedValue({ registered: true });

    const url = await createDesktopGoogleLoginUrl(
      "https://patchbay.example/",
      initiate,
    );
    const parsed = new URL(url);
    const pending = pendingHandoff();

    expect(parsed.pathname).toBe("/login");
    expect(parsed.searchParams.get("platform")).toBe("desktop");
    expect(parsed.searchParams.get("state")).toBe(pending.state);
    expect(parsed.searchParams.get("code_challenge")).toBeTruthy();
    expect(pending.verifier).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(pending.expiresAt).toBeGreaterThan(Date.now());
    expect(initiate).toHaveBeenCalledWith(
      pending.state,
      parsed.searchParams.get("code_challenge"),
    );
  });

  it("redeems once and makes a replay a no-op after clearing the verifier", async () => {
    const initiate = vi.fn().mockResolvedValue({ registered: true });
    const url = await createDesktopGoogleLoginUrl("https://patchbay.example", initiate);
    const state = new URL(url).searchParams.get("state");
    if (!state) throw new Error("missing handoff state");

    const redeem = vi.fn().mockResolvedValue({ token: "native-jwt" });
    const login = vi.fn().mockResolvedValue(undefined);
    const recoverPersistedToken = vi.fn();
    const dependencies = { redeem, login, recoverPersistedToken };

    await expect(
      completeDesktopHandoff("pbd_one-time-code", state, dependencies),
    ).resolves.toEqual({ acknowledged: true, authenticated: true });
    expect(redeem).toHaveBeenCalledWith(
      "pbd_one-time-code",
      expect.stringMatching(/^[A-Za-z0-9_-]{43}$/),
    );
    expect(login).toHaveBeenCalledWith("native-jwt");
    expect(localStorage.getItem(PENDING_HANDOFF_KEY)).toBeNull();

    await expect(
      completeDesktopHandoff("pbd_one-time-code", state, dependencies),
    ).resolves.toEqual({ acknowledged: true, authenticated: false });
    expect(redeem).toHaveBeenCalledTimes(1);
  });

  it("drops a terminal redeem failure so a consumed or expired code is not retried", async () => {
    const initiate = vi.fn().mockResolvedValue({ registered: true });
    const url = await createDesktopGoogleLoginUrl("https://patchbay.example", initiate);
    const state = new URL(url).searchParams.get("state");
    if (!state) throw new Error("missing handoff state");

    const redeem = vi
      .fn()
      .mockRejectedValue(new ApiError("invalid desktop auth handoff", 401, "Unauthorized"));

    await expect(
      completeDesktopHandoff("pbd_consumed-code", state, {
        redeem,
        login: vi.fn(),
        recoverPersistedToken: vi.fn(),
      }),
    ).resolves.toEqual({ acknowledged: true, authenticated: false });
    expect(localStorage.getItem(PENDING_HANDOFF_KEY)).toBeNull();
  });
});
