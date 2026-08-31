import { afterEach, describe, expect, it, vi } from "vitest";
import { AUTH_CONTRACT_HEADER } from "./contract";
import {
  completeDesktopGoogleAttempt,
  registerDesktopGoogleAttempt,
} from "./broker-client";

const binding = {
  state: "s".repeat(43),
  code_challenge: "c".repeat(43),
};

describe("same-origin auth broker client", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("registers the versioned PKCE binding without a bearer", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValue(Response.json({ registered: true }));
    vi.stubGlobal("fetch", fetcher);

    await registerDesktopGoogleAttempt(binding);

    const [path, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(path).toBe("/v1/desktop/google/attempt");
    expect(JSON.parse(String(init.body))).toEqual(binding);
    const headers = new Headers(init.headers);
    expect(headers.get(AUTH_CONTRACT_HEADER)).toBe("1");
    expect(headers.get("authorization")).toBeNull();
    expect(init.credentials).toBe("same-origin");
  });

  it("uses the Clerk bearer only for completion and accepts only a one-time grant", async () => {
    const code = `pbd_${"g".repeat(43)}`;
    const fetcher = vi.fn().mockResolvedValue(
      Response.json({
        callback_protocol: "patchbay-canary-login-fix-123",
        code,
      }),
    );
    vi.stubGlobal("fetch", fetcher);

    await expect(
      completeDesktopGoogleAttempt("clerk-session", binding),
    ).resolves.toEqual({
      callbackProtocol: "patchbay-canary-login-fix-123",
      code,
    });

    const [path, init] = fetcher.mock.calls[0] as [string, RequestInit];
    expect(path).toBe("/v1/desktop/google/complete");
    expect(new Headers(init.headers).get("authorization")).toBe(
      "Bearer clerk-session",
    );
  });
});
