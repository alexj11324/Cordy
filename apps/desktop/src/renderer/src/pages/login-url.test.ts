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

  it("fails closed instead of reopening the legacy accounts worker", () => {
    expect(() =>
      buildDesktopGoogleLoginUrl("https://accounts.aspectlylabs.com"),
    ).toThrow("Legacy accounts login origin is not supported");
  });
});
