"use client";
import { useEffect, useState } from "react";
import en from "../locales/en.json";
import ja from "../locales/ja.json";
import ko from "../locales/ko.json";
import zhHans from "../locales/zh-Hans.json";
import { resolveAuthLocale, type AuthLocale } from "./auth-locale";
type Messages = typeof en;
const locales: Record<AuthLocale, Messages> = {
  en,
  ja,
  ko,
  "zh-Hans": zhHans,
};

export function useAuthMessages(): Messages {
  const [messages, setMessages] = useState<Messages>(en);
  useEffect(() => {
    const { locale } = resolveAuthLocale(navigator.language);
    setMessages(locales[locale]);
  }, []);
  return messages;
}
