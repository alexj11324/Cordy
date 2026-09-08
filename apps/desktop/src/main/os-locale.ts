export type AppCopyLocale = "en" | "zh-Hans";

/**
 * Map OS-preferred language tags to the locales the desktop main process
 * ships copy for. Chinese variants all resolve to Simplified — Orvilo does
 * not ship zh-Hant, and falling through to English is worse than reading
 * Simplified for those users. Every other tag reads English.
 */
export function preferredAppLocaleFromLanguages(
  languages: readonly string[],
): AppCopyLocale {
  const preferred = languages[0]?.toLowerCase() ?? "";
  if (preferred.startsWith("zh")) return "zh-Hans";
  return "en";
}
