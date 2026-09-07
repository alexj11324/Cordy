import { describe, expect, it } from "vitest";
import {
  DEFAULT_ACCOUNTS_RETURN_URL,
  resolveAccountsReturnUrl,
  resolveStandaloneReturnUrl,
} from "./redirect";

describe("Accounts return URLs", () => {
  it("uses the runtime staging origin for defaults and rejects production returns", () => {
    const origin = "https://staging.aspectlylabs.com";
    expect(resolveAccountsReturnUrl(`${origin}/acme/issues`, origin)).toBe(`${origin}/acme/issues`);
    for (const value of [null, "/login", DEFAULT_ACCOUNTS_RETURN_URL, "https://evil.example/login", "//evil.example/login"]) {
      expect(resolveStandaloneReturnUrl(value, origin)).toBe(`${origin}/login`);
    }
    expect(resolveAccountsReturnUrl("/login?platform=desktop", origin)).toBe("/login?platform=desktop");
  });
  it("defaults direct broker login to the product login route", () => {
    expect(resolveAccountsReturnUrl(null)).toBe(DEFAULT_ACCOUNTS_RETURN_URL);
  });

  it("preserves a relative Desktop handoff route", () => {
    expect(
      resolveAccountsReturnUrl(
        "/login?platform=desktop&state=state&code_challenge=challenge",
      ),
    ).toBe("/login?platform=desktop&state=state&code_challenge=challenge");
  });

  it("allows another route on the canonical product origin", () => {
    expect(
      resolveAccountsReturnUrl("https://patchbay.aspectlylabs.com/acme/issues"),
    ).toBe("https://patchbay.aspectlylabs.com/acme/issues");
  });

  it("does not let standalone login loop back to the broker", () => {
    expect(resolveStandaloneReturnUrl("/login")).toBe(
      DEFAULT_ACCOUNTS_RETURN_URL,
    );
  });

  it("rejects external and protocol-relative redirect targets", () => {
    expect(resolveAccountsReturnUrl("https://evil.example/login")).toBe(
      DEFAULT_ACCOUNTS_RETURN_URL,
    );
    expect(resolveAccountsReturnUrl("//evil.example/login")).toBe(
      DEFAULT_ACCOUNTS_RETURN_URL,
    );
    expect(resolveAccountsReturnUrl("javascript:alert(1)")).toBe(
      DEFAULT_ACCOUNTS_RETURN_URL,
    );
  });
});
