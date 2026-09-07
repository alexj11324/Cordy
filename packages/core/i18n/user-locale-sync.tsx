"use client";

import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useAuthStore } from "../auth";
import { api } from "../api";
import { useLocaleAdapter } from "./adapter-context";
import {
  DEFAULT_LOCALE,
  SUPPORTED_LOCALES,
  type SupportedLocale,
} from "./types";

// Pulls the server-stored `user.language` into the local locale adapter on
// login. Without this, switching device (macOS → Windows, browser → desktop)
// loses the user's language preference: pickLocale only consults the local
// adapter (cookie / localStorage), never user.language.
//
// Mounts inside CoreProvider so it has access to the auth store + locale
// adapter + i18n instance. Renders nothing.
//
// Removed locales are normalized on the server as soon as the user signs in.
// After reload, pickLocale reads the freshly-persisted value from the adapter.
export function UserLocaleSync() {
  const userLanguage = useAuthStore((s) => s.user?.language ?? null);
  const adapter = useLocaleAdapter();
  const { i18n } = useTranslation();

  useEffect(() => {
    if (!userLanguage) return;

    const nextLocale = (SUPPORTED_LOCALES as readonly string[]).includes(
      userLanguage,
    )
      ? (userLanguage as SupportedLocale)
      : DEFAULT_LOCALE;
    const removedLocale = nextLocale !== userLanguage;

    if (!removedLocale && nextLocale === i18n.language) return;
    adapter.persist(nextLocale);

    if (removedLocale) {
      void api
        .updateMe({ language: nextLocale })
        .then(() => {
          if (
            nextLocale !== i18n.language &&
            typeof window !== "undefined"
          ) {
            window.location.reload();
          }
        })
        .catch(() => undefined);
      return;
    }

    if (typeof window !== "undefined") window.location.reload();
  }, [userLanguage, i18n.language, adapter]);

  return null;
}
