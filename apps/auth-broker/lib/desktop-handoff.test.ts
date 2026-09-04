import { describe, expect, it } from "vitest";
import { buildDesktopCallbackUrl, readDesktopHandoffBinding } from "./desktop-handoff";
describe("desktop handoff", () => {
  it("keeps state and PKCE binding", () => { const state = "s".repeat(43); const challenge = "c".repeat(43); expect(readDesktopHandoffBinding(new URLSearchParams({ platform: "desktop", state, code_challenge: challenge }))).toMatchObject({ state, codeChallenge: challenge }); });
  it("rejects app_origin and unsafe protocols", () => { expect(readDesktopHandoffBinding(new URLSearchParams({ platform: "desktop", state: "s".repeat(43), code_challenge: "c".repeat(43), app_origin: "http://localhost" }))).toBeNull(); expect(() => buildDesktopCallbackUrl(`pbd_${"c".repeat(43)}`, "s".repeat(43), "https")).toThrow(); });
});
