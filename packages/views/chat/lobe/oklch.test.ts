// @vitest-environment node
//
// Pure colour maths — no DOM. The conversion feeds every antd token, so these
// assertions are the canonical layer for it; the component suite only needs to
// prove the bridge wires them through.

import { describe, expect, it } from "vitest";

import { oklchToHex, parseOklch, toAntdColor } from "./oklch";

describe("parseOklch", () => {
  it("parses the literal form used by the design tokens", () => {
    expect(parseOklch("oklch(0.55 0.16 255)")).toEqual({
      lightness: 0.55,
      chroma: 0.16,
      hue: 255,
    });
  });

  it("parses zero-chroma greys", () => {
    expect(parseOklch("oklch(0.922 0 0)")).toEqual({
      lightness: 0.922,
      chroma: 0,
      hue: 0,
    });
  });

  it("accepts percentage lightness and chroma", () => {
    expect(parseOklch("oklch(55% 0.16 255)")?.lightness).toBeCloseTo(0.55);
    expect(parseOklch("oklch(0.55 16% 255)")?.chroma).toBeCloseTo(0.16);
  });

  it("accepts an explicit deg unit and an alpha channel", () => {
    expect(parseOklch("oklch(0.55 0.16 255deg)")?.hue).toBe(255);
    expect(parseOklch("oklch(0.55 0.16 255 / 50%)")?.hue).toBe(255);
  });

  it("tolerates surrounding whitespace", () => {
    expect(parseOklch("  oklch(0.5 0.1 200)  ")?.hue).toBe(200);
  });

  it("returns null for non-OKLCH colour syntax", () => {
    expect(parseOklch("#3b82f6")).toBeNull();
    expect(parseOklch("rgb(59 130 246)")).toBeNull();
    expect(parseOklch("var(--brand)")).toBeNull();
    expect(parseOklch("")).toBeNull();
  });

  it("returns null for a value that never resolved past var()", () => {
    // `--sidebar-item-hover` is a color-mix() over other tokens; the browser
    // does not evaluate it for a custom property, so it arrives verbatim.
    expect(
      parseOklch("color-mix(in oklab, oklch(0.985 0 0) 94%, oklch(0.145 0 0))"),
    ).toBeNull();
  });
});

describe("oklchToHex", () => {
  it("maps the achromatic endpoints", () => {
    expect(oklchToHex({ lightness: 1, chroma: 0, hue: 0 })).toBe("#ffffff");
    expect(oklchToHex({ lightness: 0, chroma: 0, hue: 0 })).toBe("#000000");
  });

  it("maps the CSS Color 4 reference red", () => {
    // The spec's canonical OKLCH spelling of `red`.
    expect(oklchToHex({ lightness: 0.6279, chroma: 0.2577, hue: 29.23 })).toBe("#ff0000");
  });

  it("maps Orvilo's brand blue", () => {
    expect(oklchToHex({ lightness: 0.55, chroma: 0.16, hue: 255 })).toBe("#2171cc");
  });

  it("emits a six-digit lowercase hex for every channel width", () => {
    for (const hex of [
      oklchToHex({ lightness: 0.03, chroma: 0.01, hue: 10 }),
      oklchToHex({ lightness: 0.5, chroma: 0.2, hue: 120 }),
      oklchToHex({ lightness: 0.99, chroma: 0.02, hue: 300 }),
    ]) {
      expect(hex).toMatch(/^#[0-9a-f]{6}$/);
    }
  });

  it("clamps out-of-gamut chroma instead of emitting NaN", () => {
    const hex = oklchToHex({ lightness: 0.6, chroma: 0.9, hue: 140 });
    expect(hex).toMatch(/^#[0-9a-f]{6}$/);
  });
});

describe("toAntdColor", () => {
  it("converts OKLCH literals", () => {
    expect(toAntdColor("oklch(0.55 0.16 255)")).toBe("#2171cc");
  });

  it("passes through values antd already understands", () => {
    expect(toAntdColor("#3b82f6")).toBe("#3b82f6");
    expect(toAntdColor("#ffffff")).toBe("#ffffff");
    expect(toAntdColor("transparent")).toBe("transparent");
  });

  it("passes an unresolved reference through rather than turning it black", () => {
    // The bridge falls back on these; silently returning #000000 is the exact
    // failure mode this module exists to prevent.
    expect(toAntdColor("var(--brand)")).toBe("var(--brand)");
    expect(toAntdColor("color-mix(in oklab, red, blue)")).toBe(
      "color-mix(in oklab, red, blue)",
    );
  });

  it("trims surrounding whitespace", () => {
    expect(toAntdColor("  #3b82f6  ")).toBe("#3b82f6");
  });
});
