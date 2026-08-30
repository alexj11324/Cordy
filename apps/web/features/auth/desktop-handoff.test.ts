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
      appOrigin: null,
      query: `platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
    });
  });

  it("preserves a browser return only when deployment config names the exact app origin", () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    const searchParams = new URLSearchParams({
      platform: "desktop",
      code_challenge: codeChallenge,
      state,
      app_origin: "https://patchbay.aspectlylabs.com",
    });

    expect(
      readDesktopHandoffBinding(
        searchParams,
        "https://patchbay.aspectlylabs.com",
      ),
    ).toEqual({
      codeChallenge,
      state,
      appOrigin: "https://patchbay.aspectlylabs.com",
      query:
        `platform=desktop&code_challenge=${codeChallenge}` +
        `&state=${state}&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com`,
    });
  });

  it.each([
    "http://localhost:3000",
    "https://www.aspectlylabs.com",
    "https://app.patchbay.ai",
    "https://evil.example",
  ])(
    "rejects an unconfigured or mismatched browser app origin: %s",
    (appOrigin) => {
      const searchParams = new URLSearchParams({
        platform: "desktop",
        code_challenge: "a".repeat(43),
        state: "b".repeat(43),
        app_origin: appOrigin,
      });

      expect(
        readDesktopHandoffBinding(
          searchParams,
          "https://patchbay.aspectlylabs.com",
        ),
      ).toBeNull();
      expect(readDesktopHandoffBinding(searchParams)).toBeNull();
    },
  );

  it.each([
    "platform=desktop",
    `platform=desktop&code_challenge=${"a".repeat(43)}&state=short`,
    `platform=web&code_challenge=${"a".repeat(43)}&state=${"b".repeat(43)}`,
  ])("rejects an invalid provider handoff binding: %s", (query) => {
    expect(readDesktopHandoffBinding(new URLSearchParams(query))).toBeNull();
  });
});
