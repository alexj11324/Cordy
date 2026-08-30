// @vitest-environment node
import { describe, expect, it } from "vitest";

import { resolveMainWindowAppearance } from "./window-appearance";

describe("resolveMainWindowAppearance", () => {
  it("enables native sidebar vibrancy only on macOS", () => {
    expect(resolveMainWindowAppearance("darwin")).toEqual({
      transparent: true,
      backgroundColor: "#00000000",
      vibrancy: "sidebar",
      visualEffectState: "active",
    });
  });

  it.each(["win32", "linux"] as const)(
    "keeps the %s window opaque",
    (platform) => {
      expect(resolveMainWindowAppearance(platform)).toEqual({});
    },
  );
});
