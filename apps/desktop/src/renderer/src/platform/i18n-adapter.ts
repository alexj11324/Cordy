import type { LocaleAdapter, SupportedLocale } from "@patchbay/core/i18n";

const STORAGE_KEY = "patchbay-locale";
const LEGACY_STORAGE_KEY = "cordy-locale"; // legacy-brand-compat

// Desktop adapter:
//   - User choice: localStorage (set by Settings switcher).
//   - System preference: locale main injected via additionalArguments
//     (read from preload, exposed on window.desktopAPI.systemLocale).
//   - Persist: localStorage. The Settings switcher additionally PATCHes
//     /api/me when logged in so user.language follows the user across devices.
export function createDesktopLocaleAdapter(systemLocale: string): LocaleAdapter {
  return {
    getUserChoice() {
      try {
        const current = window.localStorage.getItem(STORAGE_KEY);
        if (current) return current;
        const legacy = window.localStorage.getItem(LEGACY_STORAGE_KEY);
        if (legacy) {
          window.localStorage.setItem(STORAGE_KEY, legacy);
          window.localStorage.removeItem(LEGACY_STORAGE_KEY);
        }
        return legacy;
      } catch {
        return null;
      }
    },
    getSystemPreferences() {
      return systemLocale ? [systemLocale] : [];
    },
    persist(locale: SupportedLocale) {
      try {
        window.localStorage.setItem(STORAGE_KEY, locale);
      } catch {
        // Best-effort
      }
    },
  };
}
