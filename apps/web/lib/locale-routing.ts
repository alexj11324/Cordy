import {
  matchLocale,
  SUPPORTED_LOCALES,
  type SupportedLocale,
} from "@orvilo/core/i18n";

export const ORVILO_LOCALE_HEADER = "x-orvilo-locale";

export function isSupportedLocale(
  value: string | null,
): value is SupportedLocale {
  return (
    value !== null &&
    (SUPPORTED_LOCALES as readonly string[]).includes(value)
  );
}

export function resolveLocaleFromSignals({
  cookieLocale,
  acceptLanguage,
}: {
  cookieLocale?: string | null;
  acceptLanguage?: string | null;
}): SupportedLocale {
  const candidates: string[] = [];
  if (cookieLocale) candidates.push(cookieLocale);

  for (const part of (acceptLanguage ?? "").split(",")) {
    const tag = part.split(";")[0]?.trim();
    if (tag) candidates.push(tag);
  }

  return matchLocale(candidates);
}
