// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearDesktopHandoffVerifier,
  createDesktopLoginUrl,
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
    const url = await createDesktopLoginUrl("https://accounts.aspectlylabs.com");
    const parsed = new URL(url);

    expect(parsed.origin).toBe("https://accounts.aspectlylabs.com");
    expect(parsed.pathname).toBe("/login");
    expect(parsed.searchParams.get("platform")).toBe("desktop");
    expect(parsed.searchParams.get("code_challenge")).toHaveLength(43);
    const state = parsed.searchParams.get("state");
    expect(state).toHaveLength(43);
    expect(parsed.searchParams.get("token")).toBeNull();
    expect(readDesktopHandoffVerifier(state ?? "")).toHaveLength(43);
  });

  it("does not clear the pending verifier for an unsolicited state", async () => {
    await createDesktopLoginUrl("https://accounts.aspectlylabs.com");

    expect(readDesktopHandoffVerifier("wrong-state")).toBeNull();
    const raw = localStorage.getItem("patchbay_desktop_login_handoff");
    expect(raw).not.toBeNull();
  });

  it("keeps the verifier after the renderer session is recreated", async () => {
    const url = await createDesktopLoginUrl("https://accounts.aspectlylabs.com");
    const state = new URL(url).searchParams.get("state") ?? "";

    sessionStorage.clear();

    expect(readDesktopHandoffVerifier(state)).toHaveLength(43);
  });

  it("rejects an expired verifier", async () => {
    vi.useFakeTimers();
    try {
      const url = await createDesktopLoginUrl(
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
    const url = await createDesktopLoginUrl("https://accounts.aspectlylabs.com");
    const state = new URL(url).searchParams.get("state") ?? "";

    clearDesktopHandoffVerifier(state);

    expect(readDesktopHandoffVerifier(state)).toBeNull();
  });
});
