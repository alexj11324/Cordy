/**
 * OKLCH → sRGB hex conversion.
 *
 * antd derives its hover / active / disabled colour steps by running colour
 * maths on whatever it is handed as a seed token, via `@ant-design/fast-color`.
 * That parser has no OKLCH support: `oklch(0.55 0.16 255)` resolves to black
 * rather than blue. Every colour in Orvilo's design system is an OKLCH literal
 * (see `packages/ui/styles/tokens.css`), so the conversion has to happen
 * before any token reaches antd — otherwise the whole chat surface renders in
 * greyscale.
 *
 * The transform is the standard OKLab definition (Björn Ottosson) followed by
 * the sRGB transfer function.
 */

/** A parsed OKLCH colour. */
export interface OklchColor {
  /** Perceptual lightness, 0–1. */
  lightness: number;
  /** Chroma. Unbounded in CSS; practically 0–0.4. */
  chroma: number;
  /** Hue angle in degrees. */
  hue: number;
}

/**
 * Matches the OKLCH forms our tokens use, plus the percentage and alpha
 * variants a hand-written value might carry. Deliberately strict: a value that
 * does not match is rejected rather than guessed at.
 */
const OKLCH_PATTERN =
  /^oklch\(\s*(-?[\d.]+)(%?)\s+(-?[\d.]+)(%?)\s+(-?[\d.]+)(?:deg)?\s*(?:\/\s*[\d.]+%?\s*)?\)$/i;

/**
 * Parses an OKLCH literal. Returns null for anything that is not one — hex,
 * `rgb()`, `color-mix()`, or a `var()` that never resolved.
 */
export function parseOklch(value: string): OklchColor | null {
  const match = OKLCH_PATTERN.exec(value.trim());
  if (!match) return null;

  const [, rawLightness, lightnessUnit, rawChroma, chromaUnit, rawHue] = match;
  const lightness = Number(rawLightness) / (lightnessUnit === "%" ? 100 : 1);
  const chroma = Number(rawChroma) / (chromaUnit === "%" ? 100 : 1);
  const hue = Number(rawHue);

  if (!Number.isFinite(lightness) || !Number.isFinite(chroma) || !Number.isFinite(hue)) {
    return null;
  }

  return { lightness, chroma, hue };
}

/** sRGB transfer function: linear light → gamma-encoded. */
function encodeSrgb(linear: number): number {
  return linear <= 0.0031308 ? 12.92 * linear : 1.055 * linear ** (1 / 2.4) - 0.055;
}

function toByte(channel: number): number {
  return Math.max(0, Math.min(255, Math.round(channel * 255)));
}

/** Converts a parsed OKLCH colour to a `#rrggbb` string. */
export function oklchToHex(color: OklchColor): string {
  const hueRadians = (color.hue * Math.PI) / 180;
  const a = color.chroma * Math.cos(hueRadians);
  const b = color.chroma * Math.sin(hueRadians);

  const lPrime = color.lightness + 0.3963377774 * a + 0.2158037573 * b;
  const mPrime = color.lightness - 0.1055613458 * a - 0.0638541728 * b;
  const sPrime = color.lightness - 0.0894841775 * a - 1.291485548 * b;

  const l = lPrime ** 3;
  const m = mPrime ** 3;
  const s = sPrime ** 3;

  const red = encodeSrgb(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s);
  const green = encodeSrgb(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s);
  const blue = encodeSrgb(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s);

  return `#${[red, green, blue]
    .map((channel) => toByte(channel).toString(16).padStart(2, "0"))
    .join("")}`;
}

/**
 * Converts a CSS colour to something antd can do maths on.
 *
 * OKLCH literals are converted. Everything else (`#rrggbb`, `rgb()`, an
 * unresolved `var()`, `color-mix()`) is returned trimmed and unchanged, so the
 * caller decides how to handle it rather than silently receiving black.
 */
export function toAntdColor(value: string): string {
  const parsed = parseOklch(value);
  return parsed ? oklchToHex(parsed) : value.trim();
}
