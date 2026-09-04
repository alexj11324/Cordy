import { describe, expect, it } from "vitest";
import {
  buildDesktopCallbackUrl,
  readDesktopHandoffBinding,
} from "./desktop-handoff";

describe("desktop handoff", () => {
  it("keeps state and PKCE binding", () => {
    const state = "s".repeat(43);
    const challenge = "c".repeat(43);
    expect(
      readDesktopHandoffBinding(
        new URLSearchParams({
          platform: "desktop",
          state,
          code_challenge: challenge,
        }),
      ),
    ).toMatchObject({ state, codeChallenge: challenge, sessionApi: null });
  });

  it("preserves an allowlisted loopback session API", () => {
    const state = "s".repeat(43);
    const challenge = "c".repeat(43);
    const binding = readDesktopHandoffBinding(
      new URLSearchParams({
        platform: "desktop",
        state,
        code_challenge: challenge,
        session_api: "http://localhost:8080/",
      }),
    );
    expect(binding).toMatchObject({
      sessionApi: "http://localhost:8080",
    });
    expect(binding?.query).toContain("session_api=http%3A%2F%2Flocalhost%3A8080");
  });

  it("drops a remote session API instead of forwarding it", () => {
    const binding = readDesktopHandoffBinding(
      new URLSearchParams({
        platform: "desktop",
        state: "s".repeat(43),
        code_challenge: "c".repeat(43),
        session_api: "https://api.aspectlylabs.com",
      }),
    );
    expect(binding?.sessionApi).toBeNull();
    expect(binding?.query).not.toContain("session_api");
  });

  it("rejects app_origin and unsafe protocols", () => {
    expect(
      readDesktopHandoffBinding(
        new URLSearchParams({
          platform: "desktop",
          state: "s".repeat(43),
          code_challenge: "c".repeat(43),
          app_origin: "http://localhost",
        }),
      ),
    ).toBeNull();
    expect(() =>
      buildDesktopCallbackUrl(`pbd_${"c".repeat(43)}`, "s".repeat(43), "https"),
    ).toThrow();
  });
});
