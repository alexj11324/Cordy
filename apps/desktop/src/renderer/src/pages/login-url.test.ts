import { describe, expect, it } from "vitest";

import { buildDesktopLoginUrl } from "./login-url";

describe("buildDesktopLoginUrl", () => {
  it("uses the configured public accounts host", () => {
    expect(buildDesktopLoginUrl("https://accounts.aspectlylabs.com")).toBe(
      "https://accounts.aspectlylabs.com/login?platform=desktop",
    );
  });

  it("keeps explicit self-hosted app URLs configurable", () => {
    expect(buildDesktopLoginUrl("https://app.example.com")).toBe(
      "https://app.example.com/login?platform=desktop",
    );
  });
});
