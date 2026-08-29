import { describe, expect, it } from "vitest";
import { parseAuthDeepLinkCode } from "./auth-deep-link";

describe("parseAuthDeepLinkCode", () => {
  it("accepts the exact one-time-code callback shape", () => {
    expect(parseAuthDeepLinkCode("patchbay://auth/callback?code=opaque-code")).toBe(
      "opaque-code",
    );
  });

  it("rejects bearer credentials and ambiguous callback parameters", () => {
    for (const value of [
      "patchbay://auth/callback?token=bearer",
      "patchbay://auth/callback?access_token=bearer",
      "patchbay://auth/callback?id_token=bearer",
      "patchbay://auth/callback?code=opaque&token=bearer",
      "patchbay://auth/callback?code=opaque#token=bearer",
      "patchbay://attacker@auth/callback?code=opaque",
      "patchbay://auth:443/callback?code=opaque",
      "https://accounts.aspectlylabs.com/oauth/google/callback?code=opaque",
      "patchbay://auth/callback?code=not%20opaque",
    ]) {
      expect(parseAuthDeepLinkCode(value)).toBeNull();
    }
  });
});
