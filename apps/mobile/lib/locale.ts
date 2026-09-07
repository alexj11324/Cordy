/**
 * The product locales and the one place that maps an account `language`
 * onto them.
 *
 * Account language arrives in whatever shape the platform reported it —
 * `zh-CN`, `zh_Hant`, mixed case, stray whitespace. Copy modules used to carry
 * their own normalizer and had already drifted apart on which shapes counted as
 * Chinese. They all take this one instead.
 *
 * Anything that is not Chinese resolves to English, including the retired `ja`
 * and `ko` tags that existing accounts may still have stored.
 */
export type ProductLocale = "en" | "zh-Hans";

export const PRODUCT_LOCALES: ProductLocale[] = ["en", "zh-Hans"];

export function normalizeProductLocale(
  language: string | null | undefined,
): ProductLocale {
  const normalized = language?.trim().toLowerCase().replaceAll("_", "-");
  if (normalized?.startsWith("zh")) return "zh-Hans";
  return "en";
}
