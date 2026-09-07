// @vitest-environment node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(process.cwd(), "../..");
const chineseFonts = ["PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC"];
const koreanFonts = ["Apple SD Gothic Neo", "Malgun Gothic", "Noto Sans CJK KR"];
const japaneseFonts = ["Hiragino Sans", "Yu Gothic", "Noto Sans CJK JP"];

function expectChineseFontsBeforeKoreanFonts(source: string) {
  const chineseIndexes = chineseFonts.map((font) => source.indexOf(font));
  const koreanIndexes = koreanFonts.map((font) => source.indexOf(font));

  expect(chineseIndexes).not.toContain(-1);
  expect(koreanIndexes).not.toContain(-1);

  for (const chineseIndex of chineseIndexes) {
    for (const koreanIndex of koreanIndexes) {
      expect(chineseIndex).toBeLessThan(koreanIndex);
    }
  }
}

// The UI ships English and Chinese only, but members still write Japanese into
// issues and comments, so the stack must keep Japanese coverage. Kana has its
// own Unicode block and renders natively from these families; shared Han
// ideographs stay Chinese-first, the same tradeoff already made for Korean.
// No lang-scoped override survives: <html lang> is only ever en or zh-CN.
function expectJapaneseFontsAfterChinese(source: string) {
  expect(source).not.toContain('html[lang|="ja"]');

  const japaneseIndexes = japaneseFonts.map((font) => source.indexOf(font));
  const chineseIndexes = chineseFonts.map((font) => source.indexOf(font));

  expect(japaneseIndexes).not.toContain(-1);
  expect(chineseIndexes).not.toContain(-1);

  for (const chineseIndex of chineseIndexes) {
    for (const japaneseIndex of japaneseIndexes) {
      expect(chineseIndex).toBeLessThan(japaneseIndex);
    }
  }
}

describe("CJK font fallback order", () => {
  it("keeps web Chinese font fallbacks before Korean font fallbacks", () => {
    const cssSource = readFileSync(
      resolve(repoRoot, "apps/web/app/globals.css"),
      "utf8",
    );

    expectChineseFontsBeforeKoreanFonts(cssSource);
  });

  it("keeps Japanese font fallbacks after Chinese (web)", () => {
    const cssSource = readFileSync(
      resolve(repoRoot, "apps/web/app/globals.css"),
      "utf8",
    );

    expectJapaneseFontsAfterChinese(cssSource);
  });

  it("keeps desktop Chinese font fallbacks before Korean font fallbacks", () => {
    const desktopCss = readFileSync(
      resolve(repoRoot, "apps/desktop/src/renderer/src/globals.css"),
      "utf8",
    );

    expectChineseFontsBeforeKoreanFonts(desktopCss);
  });

  it("keeps Japanese font fallbacks after Chinese (desktop)", () => {
    const desktopCss = readFileSync(
      resolve(repoRoot, "apps/desktop/src/renderer/src/globals.css"),
      "utf8",
    );

    expectJapaneseFontsAfterChinese(desktopCss);
  });
});
