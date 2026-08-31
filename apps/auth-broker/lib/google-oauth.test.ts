import { describe, expect, it, vi } from "vitest";
import {
  consumeGoogleOAuthNonce,
  googleOAuthAttemptIsReady,
  hasClerkOAuthReturn,
  readGoogleSso,
  startGoogleOAuth,
  withGoogleOAuthStartTimeout,
  GoogleOAuthStartTimeoutError,
} from "./google-oauth";

describe("Clerk Core 3 Google OAuth adapter", () => {
  it("fails a stuck pre-redirect operation without delaying immediate SSO", async () => {
    vi.useFakeTimers();
    try {
      const pending = new Promise<never>(() => undefined);
      const result = withGoogleOAuthStartTimeout(pending, 25);
      const rejection = expect(result).rejects.toBeInstanceOf(
        GoogleOAuthStartTimeoutError,
      );
      await vi.advanceTimersByTimeAsync(25);
      await rejection;
    } finally {
      vi.useRealTimers();
    }

    await expect(
      withGoogleOAuthStartTimeout(Promise.resolve("started"), 10_000),
    ).resolves.toBe("started");
  });

  it("starts SSO with absolute broker-owned URLs and account selection", async () => {
    const sso = vi.fn().mockResolvedValue({ error: null });

    await startGoogleOAuth({ sso }, {
      returnUrl: `/login?state=${"s".repeat(43)}`,
      callbackUrl: `/oauth/google/callback?state=${"s".repeat(43)}`,
      origin: "https://accounts.aspectlylabs.com",
    });

    expect(sso).toHaveBeenCalledWith({
      strategy: "oauth_google",
      redirectUrl: `https://accounts.aspectlylabs.com/login?state=${"s".repeat(43)}`,
      redirectCallbackUrl:
        `https://accounts.aspectlylabs.com/oauth/google/callback?state=${"s".repeat(43)}`,
      oidcPrompt: "select_account",
    });
  });

  it("supports Clerk's public and transitional Core 3 SSO surfaces", () => {
    expect(readGoogleSso({ sso: vi.fn() })).toBeTypeOf("function");
    expect(readGoogleSso({ __internal_future: { sso: vi.fn() } })).toBeTypeOf(
      "function",
    );
    expect(readGoogleSso({})).toBeNull();
  });

  it("detects OAuth returns and waits for a real attempt result", () => {
    expect(hasClerkOAuthReturn(new URLSearchParams("__clerk_status=verified"))).toBe(
      true,
    );
    expect(hasClerkOAuthReturn(new URLSearchParams(), "#__clerk_ticket=1")).toBe(
      true,
    );
    expect(googleOAuthAttemptIsReady({ status: "complete" }, {})).toBe(true);
    expect(googleOAuthAttemptIsReady({ status: null }, {})).toBe(false);
  });

  it("consumes the rotating token nonce before finalization", async () => {
    const reload = vi.fn().mockResolvedValue(undefined);
    await expect(consumeGoogleOAuthNonce({ reload }, "nonce-1")).resolves.toBe(true);
    expect(reload).toHaveBeenCalledWith({ rotatingTokenNonce: "nonce-1" });
    await expect(consumeGoogleOAuthNonce({}, "nonce-1")).resolves.toBe(false);
  });
});
