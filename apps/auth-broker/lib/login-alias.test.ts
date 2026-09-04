import { describe, expect, it } from "vitest";
import { loginAliasDestination } from "./login-alias";

describe("loginAliasDestination", () => {
  it("canonicalizes the legacy path without dropping desktop handoff parameters", () => {
    expect(
      loginAliasDestination({
        platform: "desktop",
        state: "state",
        code_challenge: "challenge",
        redirect_url: "https://patchbay.aspectlylabs.com/",
      }),
    ).toBe(
      "/login?platform=desktop&state=state&code_challenge=challenge&redirect_url=https%3A%2F%2Fpatchbay.aspectlylabs.com%2F",
    );
  });

  it("supports repeated query parameters and an empty query", () => {
    expect(loginAliasDestination({ next: ["/a", "/b"], ignored: undefined })).toBe(
      "/login?next=%2Fa&next=%2Fb",
    );
    expect(loginAliasDestination({})).toBe("/login");
  });
});
