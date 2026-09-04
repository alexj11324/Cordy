export type AppCopyLocale = "en" | "zh-Hans" | "ja" | "ko";

/**
 * Map OS-preferred language tags to the four locales the desktop main
 * process ships copy for. Chinese variants all resolve to Simplified —
 * Patchbay does not ship zh-Hant, and falling through to English is worse
 * than reading Simplified for those users.
 */
export function preferredAppLocaleFromLanguages(
  languages: readonly string[],
): AppCopyLocale {
  const preferred = languages[0]?.toLowerCase() ?? "";
  if (preferred.startsWith("zh")) return "zh-Hans";
  if (preferred.startsWith("ja")) return "ja";
  if (preferred.startsWith("ko")) return "ko";
  return "en";
}
