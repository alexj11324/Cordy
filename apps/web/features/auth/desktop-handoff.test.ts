import { describe, expect, it } from "vitest";
import {
  buildDesktopHandoffQuery,
  readDesktopHandoffBinding,
} from "./desktop-handoff";

describe("buildDesktopHandoffQuery", () => {
  it("preserves both PKCE binding parameters", () => {
    const query = buildDesktopHandoffQuery(
      new URLSearchParams(
        "platform=desktop&code_challenge=challenge-value&state=opaque-state",
      ),
    );

    expect(query).toBe(
      "platform=desktop&code_challenge=challenge-value&state=opaque-state",
    );
  });

  it("does not copy unrelated callback parameters", () => {
    const query = buildDesktopHandoffQuery(
      new URLSearchParams(
        "platform=desktop&code_challenge=challenge-value&state=opaque-state&token=secret",
      ),
    );

    expect(query).toBe(
      "platform=desktop&code_challenge=challenge-value&state=opaque-state",
    );
  });

  it("accepts a complete renderer-generated desktop binding", () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);

    expect(
      readDesktopHandoffBinding(
        new URLSearchParams({
          platform: "desktop",
          code_challenge: codeChallenge,
          state,
          token: "must-not-be-forwarded",
        }),
      ),
    ).toEqual({
      codeChallenge,
      state,
      query: `platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
    });
  });

  it("rejects the retired HTTP app-origin transport", () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    const searchParams = new URLSearchParams({
      platform: "desktop",
      code_challenge: codeChallenge,
      state,
      app_origin: "https://patchbay.aspectlylabs.com",
    });

    expect(readDesktopHandoffBinding(searchParams)).toBeNull();
  });

  it.each([
    "platform=desktop",
    `platform=desktop&code_challenge=${"a".repeat(43)}&state=short`,
    `platform=web&code_challenge=${"a".repeat(43)}&state=${"b".repeat(43)}`,
  ])("rejects an invalid provider handoff binding: %s", (query) => {
    expect(readDesktopHandoffBinding(new URLSearchParams(query))).toBeNull();
  });
});
