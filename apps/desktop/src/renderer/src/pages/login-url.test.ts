import { describe, expect, it } from "vitest";

import { buildDesktopGoogleLoginUrl } from "./login-url";

describe("buildDesktopGoogleLoginUrl", () => {
  it("uses the configured Patchbay web origin", () => {
    expect(buildDesktopGoogleLoginUrl("https://patchbay.ai")).toBe(
      "https://patchbay.ai/oauth/google?platform=desktop",
    );
  });

  it("keeps explicit self-hosted app URLs configurable", () => {
    expect(buildDesktopGoogleLoginUrl("https://app.example.com")).toBe(
      "https://app.example.com/oauth/google?platform=desktop",
    );
  });

  it("carries an explicit browser app origin without assuming localhost", () => {
    expect(
      buildDesktopGoogleLoginUrl(
        "https://accounts.patchbay.ai",
        "https://app.patchbay.ai",
      ),
    ).toBe(
      "https://accounts.patchbay.ai/oauth/google?platform=desktop&app_origin=https%3A%2F%2Fapp.patchbay.ai",
    );
  });

  it("rejects a browser return URL that is not an exact origin", () => {
    expect(() =>
      buildDesktopGoogleLoginUrl(
        "https://accounts.patchbay.ai",
        "https://app.patchbay.ai/auth/callback",
      ),
    ).toThrow("Desktop browser return origin must be an HTTP(S) origin");
  });

  it("fails closed instead of reopening the legacy accounts worker", () => {
    expect(() =>
      buildDesktopGoogleLoginUrl("https://accounts.aspectlylabs.com"),
    ).toThrow("Legacy accounts login origin is not supported");
  });
});
