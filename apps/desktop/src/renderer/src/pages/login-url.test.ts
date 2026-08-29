import { describe, expect, it } from "vitest";

import { buildDesktopGoogleLoginUrl } from "./login-url";

describe("desktop auth URL builders", () => {
  it("uses the configured public accounts host", () => {
    expect(buildDesktopGoogleLoginUrl("https://accounts.aspectlylabs.com")).toBe(
      "https://accounts.aspectlylabs.com/oauth/google?platform=desktop",
    );
  });

  it("keeps explicit self-hosted app URLs configurable", () => {
    expect(buildDesktopGoogleLoginUrl("https://app.example.com")).toBe(
      "https://app.example.com/oauth/google?platform=desktop",
    );
  });
});
