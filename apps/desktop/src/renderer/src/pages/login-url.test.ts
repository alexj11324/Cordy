import { describe, expect, it } from "vitest";

import { buildDesktopGoogleLoginUrl } from "./login-url";

describe("buildDesktopGoogleLoginUrl", () => {
  it("uses the configured accounts origin", () => {
    expect(buildDesktopGoogleLoginUrl("https://accounts.aspectlylabs.com")).toBe(
      "https://accounts.aspectlylabs.com/oauth/google?platform=desktop",
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
        "https://accounts.aspectlylabs.com",
        "https://patchbay.aspectlylabs.com",
      ),
    ).toBe(
      "https://accounts.aspectlylabs.com/oauth/google?platform=desktop&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com",
    );
  });

  it("rejects a browser return URL that is not an exact origin", () => {
    expect(() =>
      buildDesktopGoogleLoginUrl(
        "https://accounts.aspectlylabs.com",
        "https://patchbay.aspectlylabs.com/auth/callback",
      ),
    ).toThrow("Desktop browser return origin must be an HTTP(S) origin");
  });

  it("keeps an operator-provided broker origin configurable", () => {
    expect(buildDesktopGoogleLoginUrl("https://accounts.example.com")).toBe(
      "https://accounts.example.com/oauth/google?platform=desktop",
    );
  });
});
