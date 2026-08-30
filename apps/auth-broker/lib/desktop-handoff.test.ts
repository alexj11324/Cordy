import { describe, expect, it } from "vitest";
import {
  buildDesktopCallbackUrl,
  isDesktopHandoffInput,
  readDesktopHandoffBinding,
} from "./desktop-handoff";

const state = "s".repeat(43);
const codeChallenge = "c".repeat(43);
const code = `pbd_${"g".repeat(43)}`;

describe("desktop handoff contract", () => {
  it("accepts only the desktop PKCE binding and produces a canonical query", () => {
    const binding = readDesktopHandoffBinding(
      new URLSearchParams({
        platform: "desktop",
        code_challenge: codeChallenge,
        state,
      }),
    );

    expect(binding).toEqual({
      codeChallenge,
      state,
      query: `platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
    });
    expect(
      isDesktopHandoffInput({ code_challenge: codeChallenge, state }),
    ).toBe(true);
  });

  it.each([
    new URLSearchParams({ platform: "web", code_challenge: codeChallenge, state }),
    new URLSearchParams({ platform: "desktop", code_challenge: "short", state }),
    new URLSearchParams({
      platform: "desktop",
      code_challenge: codeChallenge,
      state,
      app_origin: "https://attacker.example",
    }),
  ])("rejects non-contract input", (params) => {
    expect(readDesktopHandoffBinding(params)).toBeNull();
  });

  it("returns only the one-time code and state to the desktop protocol", () => {
    const callback = new URL(buildDesktopCallbackUrl(code, state));

    expect(`${callback.protocol}//${callback.host}${callback.pathname}`).toBe(
      "patchbay://auth/callback",
    );
    expect([...callback.searchParams.keys()].sort()).toEqual(["code", "state"]);
    expect(callback.searchParams.get("code")).toBe(code);
    expect(callback.searchParams.get("state")).toBe(state);
  });

  it("rejects bearer tokens and malformed grants in the custom URL", () => {
    expect(() => buildDesktopCallbackUrl("eyJhbGciOiJIUzI1NiJ9.token", state)).toThrow(
      "invalid desktop callback",
    );
  });
});
