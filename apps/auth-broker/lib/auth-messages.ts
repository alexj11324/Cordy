"use client";
import { createContext, createElement, useContext, type ReactNode } from "react";
import { authMessages, type AuthMessages } from '@patchbay/auth-ui/messages';
import { type AuthLocale } from './auth-locale';
type Messages = AuthMessages;
const locales = authMessages;
const AuthMessagesContext = createContext<Messages>(authMessages.en);

export function AuthMessagesProvider({ locale, children }: { locale: AuthLocale; children: ReactNode }) {
  return createElement(AuthMessagesContext.Provider, { value: locales[locale] }, children);
}

export function useAuthMessages(): Messages {
  return useContext(AuthMessagesContext);
}
