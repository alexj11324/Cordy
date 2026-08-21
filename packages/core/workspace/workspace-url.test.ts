import { describe, expect, it } from "vitest";
import { workspaceUrlHost } from "./workspace-url";

describe("workspaceUrlHost", () => {
  it("returns the host of a full app URL", () => {
    expect(workspaceUrlHost("https://cordy.example.com")).toBe(
      "cordy.example.com",
    );
  });

  it("ignores scheme, path, and trailing slash", () => {
    expect(workspaceUrlHost("https://cordy.example.com/")).toBe(
      "cordy.example.com",
    );
    expect(workspaceUrlHost("http://cordy.example.com/app/onboarding")).toBe(
      "cordy.example.com",
    );
  });

  it("preserves a non-default port", () => {
    expect(workspaceUrlHost("https://my.host:3000")).toBe("my.host:3000");
  });

  it("accepts a bare host without a scheme", () => {
    expect(workspaceUrlHost("cordy.example.com")).toBe("cordy.example.com");
    expect(workspaceUrlHost("cordy.example.com/path")).toBe(
      "cordy.example.com",
    );
  });

  it("falls back to the brand host when no app URL is configured", () => {
    expect(workspaceUrlHost("")).toBe("cordy.ai");
    expect(workspaceUrlHost("   ")).toBe("cordy.ai");
    expect(workspaceUrlHost(null)).toBe("cordy.ai");
    expect(workspaceUrlHost(undefined)).toBe("cordy.ai");
  });
});
