// @vitest-environment node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(process.cwd(), "../..");

// The full Chinese set the stack declares, not a sample of it. "Hiragino Sans GB"
// belongs here and not with the Japanese families below: GB is the Simplified
// Chinese standard, and LobeHub lists it third among its Chinese faces.
const chineseFonts = [
  "HarmonyOS Sans SC",
  "PingFang SC",
  "Hiragino Sans GB",
  "Microsoft YaHei UI",
  "Microsoft YaHei",
  "Source Han Sans SC",
  "Noto Sans CJK SC",
];
const koreanFonts = ["Apple SD Gothic Neo", "Malgun Gothic", "Noto Sans CJK KR"];
const japaneseFonts = ["Hiragino Sans", "Yu Gothic", "Noto Sans CJK JP"];

/**
 * The family list the browser actually reads.
 *
 * Comments are stripped first, and the entries are compared as whole families
 * rather than as substrings of the file. Both matter: the previous version ran
 * `indexOf` over the raw text, so it counted a font name quoted inside a comment
 * as a stack entry, and `indexOf("Hiragino Sans")` matched inside
 * `"Hiragino Sans GB"` — a Chinese family, and one that sits *earlier* in
 * LobeHub's list than `"PingFang SC"`. That made the guard fail on a correct
 * stack, which is the one failure mode a guard must not have.
 */
function fontFamilies(source: string): string[] {
  const withoutComments = source.replace(/\/\*[\s\S]*?\*\//g, "");
  const declaration = withoutComments.match(/--font-sans:\s*([^;]+);/)?.[1];
  if (!declaration) throw new Error("no --font-sans declaration in this stylesheet");

  return declaration
    .split(",")
    .map((family) => family.trim().replace(/^["']|["']$/g, ""))
    .filter(Boolean);
}

function indexOfEvery(stack: string[], fonts: string[]): number[] {
  const indexes = fonts.map((font) => stack.indexOf(font));
  expect(indexes).not.toContain(-1);
  return indexes;
}

function expectEveryPair(a: number[], b: number[]) {
  for (const aIndex of a) {
    for (const bIndex of b) {
      expect(aIndex).toBeLessThan(bIndex);
    }
  }
}

// The UI ships English and Chinese only, but members still write Japanese into
// issues and comments, so the stack must keep Japanese coverage. Kana has its
// own Unicode block and renders natively from these families; shared Han
// ideographs stay Chinese-first, the same tradeoff already made for Korean.
// No lang-scoped override survives: <html lang> is only ever en or zh-CN.
function expectChineseBeforeJapanese(source: string) {
  expect(source).not.toContain('html[lang|="ja"]');

  const stack = fontFamilies(source);
  expectEveryPair(
    indexOfEvery(stack, chineseFonts),
    indexOfEvery(stack, japaneseFonts),
  );
}

function expectChineseBeforeKorean(source: string) {
  const stack = fontFamilies(source);
  expectEveryPair(
    indexOfEvery(stack, chineseFonts),
    indexOfEvery(stack, koreanFonts),
  );
}

const WEB_CSS = "apps/web/app/globals.css";
const DESKTOP_CSS = "apps/desktop/src/renderer/src/globals.css";

const readCss = (path: string) => readFileSync(resolve(repoRoot, path), "utf8");

describe("CJK font fallback order", () => {
  it("keeps web Chinese font fallbacks before Korean font fallbacks", () => {
    expectChineseBeforeKorean(readCss(WEB_CSS));
  });

  it("keeps Japanese font fallbacks after Chinese (web)", () => {
    expectChineseBeforeJapanese(readCss(WEB_CSS));
  });

  it("keeps desktop Chinese font fallbacks before Korean font fallbacks", () => {
    expectChineseBeforeKorean(readCss(DESKTOP_CSS));
  });

  it("keeps Japanese font fallbacks after Chinese (desktop)", () => {
    expectChineseBeforeJapanese(readCss(DESKTOP_CSS));
  });
});
