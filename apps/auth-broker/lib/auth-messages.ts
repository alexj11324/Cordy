"use client";

import { useEffect, useState } from "react";
import en from "../locales/en.json";
import ja from "../locales/ja.json";
import ko from "../locales/ko.json";
import zhHans from "../locales/zh-Hans.json";

const RESOURCES = { en, ja, ko, "zh-Hans": zhHans } as const;
type SupportedLocale = keyof typeof RESOURCES;

function resolveLocale(languages: readonly string[]): SupportedLocale {
  for (const language of languages) {
    const normalized = language.toLowerCase();
    if (normalized.startsWith("zh")) return "zh-Hans";
    if (normalized.startsWith("ja")) return "ja";
    if (normalized.startsWith("ko")) return "ko";
    if (normalized.startsWith("en")) return "en";
  }
  return "en";
}

export function useAuthMessages() {
  const [locale, setLocale] = useState<SupportedLocale>("en");
  useEffect(() => {
    const resolved = resolveLocale(navigator.languages ?? [navigator.language]);
    document.documentElement.lang = resolved;
    setLocale(resolved);
  }, []);
  return { locale, messages: RESOURCES[locale] };
}

export { resolveLocale };
