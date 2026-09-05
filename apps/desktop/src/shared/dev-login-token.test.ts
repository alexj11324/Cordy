// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
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
  it("seeds an empty storage", () => {
    const storage = memoryStorage();
    expect(seedDevLoginToken(storage, "jwt-value")).toBe(true);
    expect(storage.data[DESKTOP_TOKEN_KEY]).toBe("jwt-value");
  });

  it("never overwrites a session already in storage", () => {
    const storage = memoryStorage({ [DESKTOP_TOKEN_KEY]: "real-session" });
    expect(seedDevLoginToken(storage, "jwt-value")).toBe(false);
    expect(storage.data[DESKTOP_TOKEN_KEY]).toBe("real-session");
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
