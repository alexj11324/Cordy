import { describe, expect, it, vi } from "vitest";
import {
  canStartGoogleOAuth,
  consumeGoogleOAuthNonce,
  googleOAuthAttemptIsReady,
  googleOAuthCallbackHref,
  hasClerkOAuthReturn,
  startGoogleOAuth,
  toSameOriginUrl,
} from "./google-oauth";

describe("hasClerkOAuthReturn", () => {
  it("detects Clerk ticket parameters on the current URL", () => {
    expect(
      hasClerkOAuthReturn(
        new URLSearchParams("rotating_token_nonce=nonce-value"),
      ),
    ).toBe(true);
    expect(
      hasClerkOAuthReturn(new URLSearchParams("__clerk_status=complete")),
    ).toBe(true);
    expect(hasClerkOAuthReturn(new URLSearchParams(), "#/__clerk_status=verified")).toBe(
      true,
    );
    expect(
      hasClerkOAuthReturn(
        new URLSearchParams("platform=desktop&code_challenge=abc"),
      ),
    ).toBe(false);
  });
});

describe("toSameOriginUrl", () => {
  it("resolves broker paths against the accounts origin", () => {
    expect(
      toSameOriginUrl(
        "/oauth/google/callback?platform=desktop",
        "https://accounts.aspectlylabs.com",
      ),
    ).toBe(
      "https://accounts.aspectlylabs.com/oauth/google/callback?platform=desktop",
    );
  });
});

describe("googleOAuthCallbackHref", () => {
  it("keeps Clerk tickets and the desktop binding on the callback path", () => {
    expect(
      googleOAuthCallbackHref({
        pathname: "/oauth/google",
        search:
          "?platform=desktop&code_challenge=abc&state=def&rotating_token_nonce=nonce",
        hash: "",
      }),
    ).toBe(
      "/oauth/google/callback?platform=desktop&code_challenge=abc&state=def&rotating_token_nonce=nonce",
    );
  });
});

describe("startGoogleOAuth", () => {
  it("uses Core 3 sso when it is available", async () => {
    const sso = vi.fn().mockResolvedValue({ error: null });

    await expect(
      startGoogleOAuth(
        { sso },
        {
          returnUrl: "/login",
          callbackUrl: "/oauth/google/callback",
          origin: "https://accounts.aspectlylabs.com",
        },
      ),
    ).resolves.toEqual({ error: null });
    expect(sso).toHaveBeenCalledWith({
      strategy: "oauth_google",
      redirectUrl: "https://accounts.aspectlylabs.com/login",
      redirectCallbackUrl:
        "https://accounts.aspectlylabs.com/oauth/google/callback",
      oidcPrompt: "select_account",
    });
  });

  it("reads sso off Clerk's future resource wrapper", async () => {
    const sso = vi.fn().mockResolvedValue({ error: null });

    await startGoogleOAuth(
      { __internal_future: { sso } },
      {
        returnUrl: "/login",
        callbackUrl: "/oauth/google/callback",
        origin: "https://accounts.aspectlylabs.com",
      },
    );
    expect(sso).toHaveBeenCalledOnce();
    expect(canStartGoogleOAuth({ __internal_future: { sso } })).toBe(true);
    expect(canStartGoogleOAuth({})).toBe(false);
  });
});

describe("googleOAuthAttemptIsReady", () => {
  it("waits until Clerk has a status or transfer to consume", () => {
    expect(
      googleOAuthAttemptIsReady(
        { status: null, isTransferable: false, existingSession: null },
        { status: null, isTransferable: false, existingSession: null },
      ),
    ).toBe(false);
    expect(
      googleOAuthAttemptIsReady(
        { status: "complete", isTransferable: false, existingSession: null },
        { status: null, isTransferable: false, existingSession: null },
      ),
    ).toBe(true);
  });
});

describe("consumeGoogleOAuthNonce", () => {
  it("reloads the SignIn resource with the rotating token", async () => {
    const reload = vi.fn().mockResolvedValue(undefined);
    await expect(
      consumeGoogleOAuthNonce({ reload }, "nonce-value"),
    ).resolves.toBe(true);
    expect(reload).toHaveBeenCalledWith({ rotatingTokenNonce: "nonce-value" });
  });

  it("is ready without a nonce", async () => {
    const reload = vi.fn();
    await expect(consumeGoogleOAuthNonce({ reload }, null)).resolves.toBe(true);
    expect(reload).not.toHaveBeenCalled();
  });

  it("waits when Clerk has not exposed its reload helper yet", async () => {
    await expect(consumeGoogleOAuthNonce({}, "nonce-value")).resolves.toBe(
      false,
    );
  });
});
