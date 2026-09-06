// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  DESKTOP_SEEDED_TOKEN_KEY,
  DESKTOP_TOKEN_KEY,
  seedDevLoginToken,
  type TokenStorage,
} from "./dev-login-token";

function memoryStorage(initial?: Record<string, string>): TokenStorage & {
  data: Record<string, string>;
} {
  const data: Record<string, string> = { ...initial };
  return {
    data,
    getItem: (key) => (key in data ? data[key] : null),
    setItem: (key, value) => {
      data[key] = value;
    },
  };
}

describe("seedDevLoginToken", () => {
  it("seeds an empty storage and records what it seeded", () => {
    const storage = memoryStorage();
    expect(seedDevLoginToken(storage, "jwt-value")).toBe(true);
    expect(storage.data[DESKTOP_TOKEN_KEY]).toBe("jwt-value");
    expect(storage.data[DESKTOP_SEEDED_TOKEN_KEY]).toBe("jwt-value");
  });

  it("never overwrites a session the developer signed in with", () => {
    const storage = memoryStorage({ [DESKTOP_TOKEN_KEY]: "real-session" });
    expect(seedDevLoginToken(storage, "jwt-value")).toBe(false);
    expect(storage.data[DESKTOP_TOKEN_KEY]).toBe("real-session");
  });

  // The stale token is the common case after the local database is recreated:
  // the API rejects it, the app clears it, and this helper does not run again
  // until the next boot — so keeping it would strand Electron on the login
  // screen for a whole extra restart.
  it("replaces a token it seeded earlier", () => {
    const storage = memoryStorage({
      [DESKTOP_TOKEN_KEY]: "expired-dev-token",
      [DESKTOP_SEEDED_TOKEN_KEY]: "expired-dev-token",
    });
    expect(seedDevLoginToken(storage, "fresh-dev-token")).toBe(true);
    expect(storage.data[DESKTOP_TOKEN_KEY]).toBe("fresh-dev-token");
    expect(storage.data[DESKTOP_SEEDED_TOKEN_KEY]).toBe("fresh-dev-token");
  });

  it("keeps a real session even when one was seeded before it", () => {
    const storage = memoryStorage({
      [DESKTOP_TOKEN_KEY]: "signed-in-by-hand",
      [DESKTOP_SEEDED_TOKEN_KEY]: "an-older-dev-token",
    });
    expect(seedDevLoginToken(storage, "fresh-dev-token")).toBe(false);
    expect(storage.data[DESKTOP_TOKEN_KEY]).toBe("signed-in-by-hand");
  });

  it("does not rewrite storage when the same token is already seeded", () => {
    const storage = memoryStorage({
      [DESKTOP_TOKEN_KEY]: "jwt-value",
      [DESKTOP_SEEDED_TOKEN_KEY]: "jwt-value",
    });
    expect(seedDevLoginToken(storage, "jwt-value")).toBe(false);
  });

  it("ignores absent, blank and non-string values", () => {
    for (const value of [undefined, null, "", "   ", 42, {}]) {
      const storage = memoryStorage();
      expect(seedDevLoginToken(storage, value)).toBe(false);
      expect(storage.data[DESKTOP_TOKEN_KEY]).toBeUndefined();
    }
  });

  it("trims the value it stores", () => {
    const storage = memoryStorage();
    expect(seedDevLoginToken(storage, "  jwt-value\n")).toBe(true);
    expect(storage.data[DESKTOP_TOKEN_KEY]).toBe("jwt-value");
  });

  it("returns false instead of throwing when storage is unavailable", () => {
    const storage: TokenStorage = {
      getItem: () => {
        throw new Error("storage disabled");
      },
      setItem: () => {},
    };
    expect(seedDevLoginToken(storage, "jwt-value")).toBe(false);
  });
});
