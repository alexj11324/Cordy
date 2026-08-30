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

  it("preserves a self-hosted accounts base path", () => {
    expect(buildDesktopGoogleLoginUrl("https://example.com/patchbay/")).toBe(
      "https://example.com/patchbay/oauth/google?platform=desktop",
    );
  });

  it("keeps an operator-provided broker origin configurable", () => {
    expect(buildDesktopGoogleLoginUrl("https://accounts.example.com")).toBe(
      "https://accounts.example.com/oauth/google?platform=desktop",
    );
  });
});
