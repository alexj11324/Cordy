import { describe, expect, it } from "vitest";
import { workspaceUrlHost } from "./workspace-url";

describe("workspaceUrlHost", () => {
  it("returns the host of a full app URL", () => {
    expect(workspaceUrlHost("https://orvilo.example.com")).toBe(
      "orvilo.example.com",
    );
  });

  it("ignores scheme, path, and trailing slash", () => {
    expect(workspaceUrlHost("https://orvilo.example.com/")).toBe(
      "orvilo.example.com",
    );
    expect(workspaceUrlHost("http://orvilo.example.com/app/onboarding")).toBe(
      "orvilo.example.com",
    );
  });

  it("preserves a non-default port", () => {
    expect(workspaceUrlHost("https://my.host:3000")).toBe("my.host:3000");
  });

  it("accepts a bare host without a scheme", () => {
    expect(workspaceUrlHost("orvilo.example.com")).toBe("orvilo.example.com");
    expect(workspaceUrlHost("orvilo.example.com/path")).toBe(
      "orvilo.example.com",
    );
  });

  it("falls back to the brand host when no app URL is configured", () => {
    expect(workspaceUrlHost("")).toBe("orvilo.aspectlylabs.com");
    expect(workspaceUrlHost("   ")).toBe("orvilo.aspectlylabs.com");
    expect(workspaceUrlHost(null)).toBe("orvilo.aspectlylabs.com");
    expect(workspaceUrlHost(undefined)).toBe("orvilo.aspectlylabs.com");
  });
});
