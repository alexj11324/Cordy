export type AuthLocale = "en" | "zh-Hans" | "ko" | "ja";

export type AuthLocaleResolution = {
  locale: AuthLocale;
  htmlLang: string;
};

/** Resolve the broker's four shipped translations from Accept-Language or navigator.language. */
export function resolveAuthLocale(
  rawLanguage: string | null | undefined,
): AuthLocaleResolution {
  const language = rawLanguage?.split(",", 1)[0]?.split(";", 1)[0]?.trim().toLowerCase();

  if (language?.startsWith("zh")) {
    return { locale: "zh-Hans", htmlLang: "zh-CN" };
  }
  if (language?.startsWith("ja")) {
    return { locale: "ja", htmlLang: "ja-JP" };
  }
  if (language?.startsWith("ko")) {
    return { locale: "ko", htmlLang: "ko-KR" };
  }
  return { locale: "en", htmlLang: "en" };
}
