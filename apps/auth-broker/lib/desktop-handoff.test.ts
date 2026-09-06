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
    ).toMatchObject({ state, codeChallenge: challenge, local: false });
  });

  it("uses a local identity grant without exposing a callback origin", () => {
    const binding = readDesktopHandoffBinding(new URLSearchParams({
      platform: "desktop", state: "s".repeat(43),
      code_challenge: "c".repeat(43), session_mode: "local",
    }));
    expect(binding?.local).toBe(true);
    expect(binding?.query).toContain("session_mode=local");
    expect(binding?.query).not.toContain("session_api");
    expect(buildDesktopCallbackUrl(`pbl_${"c".repeat(43)}`, "s".repeat(43), "patchbay"))
      .toContain("patchbay://auth/callback?");
    expect(buildDesktopCallbackUrl(`pbl_${"c".repeat(43)}`, "s".repeat(43), "patchbay-canary-5718c47b86bf9ece"))
      .toContain("patchbay-canary-5718c47b86bf9ece://auth/callback?");
  });

  it.each(["http://localhost:8080", "http://127.0.0.1:19080", "https://evil.example"])(
    "rejects obsolete browser-selected token destinations: %s", (sessionApi) => {
      expect(readDesktopHandoffBinding(new URLSearchParams({
        platform: "desktop", state: "s".repeat(43),
        code_challenge: "c".repeat(43), session_api: sessionApi,
      }))).toBeNull();
    },
  );

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

it("preserves Desktop's selected language through OAuth return URLs", () => {
  const binding = readDesktopHandoffBinding(new URLSearchParams({
    platform: "desktop", state: "s".repeat(43), code_challenge: "c".repeat(43),
    locale: "zh-CN", session_mode: "local",
  }));
  expect(new URLSearchParams(binding?.query).get("locale")).toBe("zh-Hans");
});
