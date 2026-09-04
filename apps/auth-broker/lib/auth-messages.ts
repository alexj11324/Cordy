"use client";
import { createContext, createElement, useContext, type ReactNode } from "react";
import en from "../locales/en.json";
import ja from "../locales/ja.json";
import ko from "../locales/ko.json";
import zhHans from "../locales/zh-Hans.json";
import { type AuthLocale } from "./auth-locale";
type Messages = typeof en;
const locales: Record<AuthLocale, Messages> = {
  en,
  ja,
  ko,
  "zh-Hans": zhHans,
};

const AuthMessagesContext = createContext<Messages>(en);

export function AuthMessagesProvider({ locale, children }: { locale: AuthLocale; children: ReactNode }) {
  return createElement(AuthMessagesContext.Provider, { value: locales[locale] }, children);
}

export function useAuthMessages(): Messages {
  return useContext(AuthMessagesContext);
}
