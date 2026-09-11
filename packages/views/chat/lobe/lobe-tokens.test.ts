// @vitest-environment node
//
// Token mapping is pure — the DOM-reading half lives in the bridge, so this
// suite is the canonical layer for the mapping matrix.

import { describe, expect, it } from "vitest";

import {
  buildAntdTokens,
  createStaticTokenReader,
  type OrviloTokenReader,
} from "./lobe-tokens";

/** A reader over a fixed token table, so assertions name the mapping not the DOM. */
function readerFrom(tokens: Record<string, string>): OrviloTokenReader {
  return {
    color: (token) => tokens[token] ?? "",
    lengthPx: (token) => Number.parseFloat(tokens[token] ?? "0"),
    font: (token) => tokens[token] ?? "",
  };
}

describe("buildAntdTokens", () => {
  it("drives colorPrimary from --brand, not --primary", () => {
    // Orvilo's --primary is a near-black neutral from the Pulse surface system.
    // Mapping it to antd's colorPrimary would strip every accent in the panel.
    const tokens = buildAntdTokens(
      readerFrom({
        "--brand": "oklch(0.55 0.16 255)",
        "--primary": "oklch(0.205 0 0)",
      }),
    );

    expect(tokens.seed.colorPrimary).toBe("#2171cc");
  });

  it("converts OKLCH literals to hex for every colour token", () => {
    const tokens = buildAntdTokens(
      readerFrom({
        "--background": "oklch(1 0 0)",
        "--foreground": "oklch(0.145 0 0)",
        "--surface": "oklch(1 0 0)",
        "--popover": "oklch(1 0 0)",
        "--muted-foreground": "oklch(0.556 0 0)",
        "--faint-foreground": "oklch(0.606 0 0)",
        "--border": "oklch(0.922 0 0)",
        "--surface-border": "oklch(0.922 0 0)",
        "--brand": "oklch(0.55 0.16 255)",
        "--destructive": "oklch(0.577 0.245 27.325)",
        "--success": "oklch(0.7 0.15 160)",
        "--warning": "oklch(0.8 0.16 85)",
        "--info": "oklch(0.6 0.2 300)",
        "--font-sans": "Inter, sans-serif",
      }),
    );

    // fontFamily is the one string-valued token that is not a colour.
    const everyColour = [
      ...Object.entries(tokens.seed)
        .filter(([key, value]) => key !== "fontFamily" && typeof value === "string")
        .map(([, value]) => value),
      ...Object.values(tokens.map).filter((value) => typeof value === "string"),
    ];

    for (const value of everyColour) {
      expect(value).toMatch(/^#[0-9a-f]{6}$/);
    }
  });

  it("maps the surface and text ramp onto Orvilo's own tokens", () => {
    const tokens = buildAntdTokens(
      readerFrom({
        "--surface": "oklch(1 0 0)",
        "--popover": "oklch(1 0 0)",
        "--background": "oklch(1 0 0)",
        "--foreground": "oklch(0.145 0 0)",
        "--muted-foreground": "oklch(0.556 0 0)",
        "--faint-foreground": "oklch(0.606 0 0)",
        "--border": "oklch(0.922 0 0)",
        "--surface-border": "oklch(0.922 0 0)",
        "--brand": "oklch(0.55 0.16 255)",
      }),
    );

    expect(tokens.map.colorBgContainer).toBe("#ffffff");
    expect(tokens.map.colorBgElevated).toBe("#ffffff");
    // oklch(0.145 0 0) is near-black: L=0.145 → linear 0.00305 → 10/255.
    expect(tokens.map.colorText).toBe("#0a0a0a");
    expect(tokens.map.colorBorder).toBe("#e5e5e5");
    // The quiet step must stay lighter than the readable secondary step.
    expect(tokens.map.colorTextTertiary).not.toBe(tokens.map.colorTextSecondary);
  });

  it("passes radii through as unitless pixels", () => {
    const tokens = buildAntdTokens(
      readerFrom({ "--brand": "#2171cc", "--radius": "10", "--radius-lg": "12" }),
    );

    expect(tokens.seed.borderRadius).toBe(10);
    expect(tokens.map.borderRadiusLG).toBe(12);
  });

  it("keeps the font stack intact", () => {
    const tokens = buildAntdTokens(
      readerFrom({
        "--brand": "#2171cc",
        "--font-sans": "Inter, 'PingFang SC', sans-serif",
      }),
    );

    expect(tokens.seed.fontFamily).toBe("Inter, 'PingFang SC', sans-serif");
  });
});

describe("createStaticTokenReader", () => {
  it("supplies a plausible light theme for the pre-paint render", () => {
    const tokens = buildAntdTokens(createStaticTokenReader());

    expect(tokens.seed.colorPrimary).toMatch(/^#[0-9a-f]{6}$/);
    expect(tokens.seed.borderRadius).toBeGreaterThan(0);
    expect(tokens.map.colorBgContainer).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("never yields an empty string, which antd would render as a parse failure", () => {
    const tokens = buildAntdTokens(createStaticTokenReader());
    const everyValue = [...Object.values(tokens.seed), ...Object.values(tokens.map)];

    for (const value of everyValue) {
      expect(value).not.toBe("");
      expect(value).not.toBeUndefined();
    }
  });
});
