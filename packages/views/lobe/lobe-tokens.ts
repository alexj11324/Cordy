/**
 * Orvilo design tokens → antd token mapping.
 *
 * The chat panel's components come from `@lobehub/ui`, which is built on antd,
 * while the rest of the app runs on Tailwind over the CSS variables in
 * `packages/ui/styles/tokens.css`. This module is the seam: it reads those
 * variables and expresses them in antd's vocabulary so the two systems render
 * the same greys, brand colour and radii.
 *
 * Reading is injected rather than done here so the mapping stays pure — the
 * `getComputedStyle` half lives in `lobe-theme-bridge.tsx`, and this half is
 * testable without a browser.
 */

import { toAntdColor } from "./oklch";

/**
 * Reads Orvilo design tokens in the forms antd needs.
 *
 * `color` returns the resolved CSS value (an `oklch()` literal for every token
 * in our system); `lengthPx` returns an already-converted pixel number, since
 * antd's radius and spacing tokens are unitless px.
 */
export interface OrviloTokenReader {
  color: (token: string) => string;
  lengthPx: (token: string) => number;
  font: (token: string) => string;
}

/**
 * antd seed tokens — the inputs its colour algorithm derives the rest from.
 *
 * Everything here is converted out of OKLCH first: antd does its own colour
 * maths on these, and `@ant-design/fast-color` cannot read `oklch()`.
 */
export interface AntdSeedTokens {
  colorPrimary: string;
  colorBgBase: string;
  colorTextBase: string;
  colorError: string;
  colorSuccess: string;
  colorWarning: string;
  colorInfo: string;
  borderRadius: number;
  fontFamily: string;
}

/**
 * antd map tokens — normally derived from the seeds.
 *
 * We pin the surfaces and the text ramp to Orvilo's own tokens instead, so the
 * chat panel sits on the same greys as the surrounding app rather than on
 * antd's neutral ramp, which drifts visibly in dark mode.
 */
export interface AntdMapTokens {
  colorBgContainer: string;
  colorBgElevated: string;
  colorBgLayout: string;
  colorText: string;
  colorTextSecondary: string;
  colorTextTertiary: string;
  colorTextDescription: string;
  colorBorder: string;
  colorBorderSecondary: string;
  borderRadiusLG: number;
}

export interface OrviloAntdTokens {
  seed: AntdSeedTokens;
  map: AntdMapTokens;
}

/**
 * Builds the token set handed to antd's `ConfigProvider`.
 *
 * `--brand` (not `--primary`) drives `colorPrimary`: Orvilo's `--primary` is a
 * near-black neutral from the Pulse surface system, so using it would leave
 * every accent, link and focus ring in the chat panel colourless.
 */
export function buildAntdTokens(read: OrviloTokenReader): OrviloAntdTokens {
  return {
    seed: {
      colorPrimary: toAntdColor(read.color("--brand")),
      colorBgBase: toAntdColor(read.color("--background")),
      colorTextBase: toAntdColor(read.color("--foreground")),
      colorError: toAntdColor(read.color("--destructive")),
      colorSuccess: toAntdColor(read.color("--success")),
      colorWarning: toAntdColor(read.color("--warning")),
      colorInfo: toAntdColor(read.color("--info")),
      borderRadius: read.lengthPx("--radius"),
      fontFamily: read.font("--font-sans"),
    },
    map: {
      colorBgContainer: toAntdColor(read.color("--surface")),
      colorBgElevated: toAntdColor(read.color("--popover")),
      colorBgLayout: toAntdColor(read.color("--background")),
      colorText: toAntdColor(read.color("--foreground")),
      colorTextSecondary: toAntdColor(read.color("--muted-foreground")),
      colorTextTertiary: toAntdColor(read.color("--faint-foreground")),
      // Pinned because Lobe's form descriptions read it directly —
      // `@lobehub/ui/es/Form/style.mjs` colours `desc` with
      // `cssVar.colorTextDescription`. Left unset, antd aliases it to
      // `colorTextTertiary`, which is `--faint-foreground` above: a token
      // documented in tokens.css as the quiet step for marks that are *not*
      // text. The `desc` under a form row is text, and it measured 130,130,130
      // on white (3.84:1) that way — below WCAG AA. `--muted-foreground` is the
      // step this app keeps for exactly this role and clears 4.5:1.
      colorTextDescription: toAntdColor(read.color("--muted-foreground")),
      colorBorder: toAntdColor(read.color("--border")),
      colorBorderSecondary: toAntdColor(read.color("--surface-border")),
      borderRadiusLG: read.lengthPx("--radius-lg"),
    },
  };
}

/**
 * Fallback reader for environments with no computed styles — SSR, and any
 * render that runs before the first paint.
 *
 * The values are Orvilo's light-theme defaults. They only have to be
 * plausible: the bridge re-reads on mount and after every theme flip, so a
 * first paint with these is corrected before it is visible.
 */
export function createStaticTokenReader(): OrviloTokenReader {
  const colors: Record<string, string> = {
    "--background": "#ffffff",
    "--foreground": "#0a0a0a",
    "--surface": "#ffffff",
    "--popover": "#ffffff",
    "--muted-foreground": "#8a8a8a",
    "--faint-foreground": "#969696",
    "--border": "#e5e5e5",
    "--surface-border": "#e5e5e5",
    "--brand": "#2171cc",
    "--destructive": "#e7000b",
    "--success": "#00bc7d",
    "--warning": "#efb100",
    "--info": "#8d54ff",
  };
  const lengths: Record<string, number> = {
    "--radius": 10,
    "--radius-lg": 10,
  };

  return {
    color: (token) => colors[token] ?? "#000000",
    lengthPx: (token) => lengths[token] ?? 10,
    font: () => "system-ui, sans-serif",
  };
}
