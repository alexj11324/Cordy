import { describe, expect, it } from "vitest";
import { resolveDevAcceptanceCdpPort } from "./dev-acceptance-cdp";

describe("resolveDevAcceptanceCdpPort", () => {
  it("keeps the endpoint disabled for normal and packaged launches", () => {
    expect(
      resolveDevAcceptanceCdpPort({
        isDev: true,
        isPackaged: false,
        enabled: undefined,
        port: "42001",
      }),
    ).toBeNull();
    expect(
      resolveDevAcceptanceCdpPort({
        isDev: false,
        isPackaged: false,
        enabled: "1",
        port: "42001",
      }),
    ).toBeNull();
    expect(
      resolveDevAcceptanceCdpPort({
        isDev: true,
        isPackaged: true,
        enabled: "1",
        port: "42001",
      }),
    ).toBeNull();
  });

  it("accepts only an explicit high-level loopback port", () => {
    expect(
      resolveDevAcceptanceCdpPort({
        isDev: true,
        isPackaged: false,
        enabled: "1",
        port: "42001",
      }),
    ).toBe(42001);
    for (const port of ["", "abc", "1023", "65536", "42001.5"]) {
      expect(() =>
        resolveDevAcceptanceCdpPort({
          isDev: true,
          isPackaged: false,
          enabled: "1",
          port,
        }),
      ).toThrow(/between 1024 and 65535/);
    }
  });
});
