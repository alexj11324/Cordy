/**
 * The four product locales and the one place that maps an account `language`
 * onto them.
 *
 * Account language arrives in whatever shape the platform reported it —
 * `zh-CN`, `ja_JP`, `ko-KR`, mixed case. Each copy module used to carry its own
 * normalizer, and they had already drifted: some matched `ja` exactly and so
 * silently served English to a `ja-JP` account. New copy modules take this one.
 */
export type ProductLocale = "en" | "zh-Hans" | "ja" | "ko";

export const PRODUCT_LOCALES: ProductLocale[] = ["en", "zh-Hans", "ja", "ko"];

export function normalizeProductLocale(
  language: string | null | undefined,
): ProductLocale {
  const normalized = language?.trim().toLowerCase().replaceAll("_", "-");
  if (normalized?.startsWith("zh")) return "zh-Hans";
  if (normalized?.startsWith("ja")) return "ja";
  if (normalized?.startsWith("ko")) return "ko";
  return "en";
}
