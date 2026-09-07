export type AuthLocale = "en" | "zh-Hans";

export type AuthLocaleResolution = {
  locale: AuthLocale;
  htmlLang: string;
};

/** Resolve the broker's shipped translations from Accept-Language or navigator.language. */
export function resolveAuthLocale(
  rawLanguage: string | null | undefined,
): AuthLocaleResolution {
  const language = rawLanguage?.split(",", 1)[0]?.split(";", 1)[0]?.trim().toLowerCase();

  if (language?.startsWith("zh")) {
    return { locale: "zh-Hans", htmlLang: "zh-CN" };
  }
  return { locale: "en", htmlLang: "en" };
}
