import { describe, expect, it } from "vitest";
import { workspaceUrlHost } from "./workspace-url";

describe("workspaceUrlHost", () => {
  it("returns the host of a full app URL", () => {
    expect(workspaceUrlHost("https://patchbay.example.com")).toBe(
      "patchbay.example.com",
    );
  });

  it("ignores scheme, path, and trailing slash", () => {
    expect(workspaceUrlHost("https://patchbay.example.com/")).toBe(
      "patchbay.example.com",
    );
    expect(workspaceUrlHost("http://patchbay.example.com/app/onboarding")).toBe(
      "patchbay.example.com",
    );
  });

  it("preserves a non-default port", () => {
    expect(workspaceUrlHost("https://my.host:3000")).toBe("my.host:3000");
  });

  it("accepts a bare host without a scheme", () => {
    expect(workspaceUrlHost("patchbay.example.com")).toBe("patchbay.example.com");
    expect(workspaceUrlHost("patchbay.example.com/path")).toBe(
      "patchbay.example.com",
    );
  });

  it("falls back to the brand host when no app URL is configured", () => {
    expect(workspaceUrlHost("")).toBe("aspectlylabs.com");
    expect(workspaceUrlHost("   ")).toBe("aspectlylabs.com");
    expect(workspaceUrlHost(null)).toBe("aspectlylabs.com");
    expect(workspaceUrlHost(undefined)).toBe("aspectlylabs.com");
  });
});
