"use client";

import { useMemo, type CSSProperties } from "react";
import { toast } from "sonner";
import { useTheme } from "@orvilo/ui/components/common/theme-provider";
import {
  DEFAULT_LOCALE,
  SUPPORTED_LOCALES,
  type SupportedLocale,
} from "@orvilo/core/i18n";
import { useLocaleAdapter } from "@orvilo/core/i18n/react";
import { useAuthStore } from "@orvilo/core/auth";
import { useCommentComposerStore } from "@orvilo/core/issues/stores";
import { api } from "@orvilo/core/api";
import { browserTimezone, timezoneOptions } from "../../common/timezone-select";
import { useT } from "../../i18n";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSelect } from "./settings-select";
import { SettingsSwitch } from "./settings-switch";

/**
 * Preferences — theme, language, timezone and the sticky comment bar.
 *
 * Every control here is **immediate-effect**: its value *is* the persisted
 * value, so none of them carries a `name` and nothing about them lives in an
 * antd `Form` store. The theme select writes the theme provider, the language
 * select persists a cookie and PATCHes `/api/me`, the timezone select PATCHes
 * and pushes the response into the auth store, and the sticky bar toggles a
 * Zustand store. The `Form` surrounding them is the shell's `Form.Group` /
 * `Form.Item` pair used purely for layout — see the state-ownership rule in
 * `reference-lobe-tab-migration.md`.
 *
 * The title and the page description that used to sit on `SettingsTab` are gone
 * on purpose: the settings dialog's own header renders them from the tab's
 * entry in `settings-page.tsx`, and the standalone suites render this component
 * without the dialog, so a title here would be the second copy of the same
 * sentence.
 */

/** The old `SettingsRow`'s `select` tier, in pixels. */
const COMPACT_SELECT_MIN_WIDTH = 192;
/** The old `SettingsRow`'s `select-wide` tier, for the IANA timezone list. */
const WIDE_SELECT_MIN_WIDTH = 288;

export function PreferencesTab() {
  const { theme, setTheme } = useTheme();
  const { t, i18n } = useT("settings");
  const localeAdapter = useLocaleAdapter();
  const user = useAuthStore((s) => s.user);

  // i18next.language can be a region-tagged BCP-47 string (e.g. "en-US",
  // "zh-Hans-CN") returned by intl-localematcher. Normalize to a supported
  // locale before comparing — otherwise the select shows neither option active.
  const currentLocale: SupportedLocale = SUPPORTED_LOCALES.includes(
    i18n.language as SupportedLocale,
  )
    ? (i18n.language as SupportedLocale)
    : DEFAULT_LOCALE;

  const themeOptions = [
    { value: "light" as const, label: t(($) => $.preferences.theme.light) },
    { value: "dark" as const, label: t(($) => $.preferences.theme.dark) },
    { value: "system" as const, label: t(($) => $.preferences.theme.system) },
  ];

  // `next-themes` reports `undefined` until it has read the stored preference
  // on the client. The select needs a value to render, and "system" is what the
  // provider itself falls back to, so the two agree either way.
  const currentTheme = theme ?? "system";

  const languageOptions: { value: SupportedLocale; label: string }[] = [
    { value: "en", label: t(($) => $.preferences.language.english) },
    { value: "zh-Hans", label: t(($) => $.preferences.language.chinese) },
  ];

  // Persist locally → sync to user.language → reload. Reload (vs in-place
  // changeLanguage) avoids hydration mismatch and is the i18next-recommended
  // pattern for App Router.
  //
  // If the cross-device sync (PATCH /api/me) fails, the local cookie is
  // already written so the new locale will take effect after reload — but
  // the user's other devices won't see the change. Surface that explicitly
  // via a toast and delay the reload long enough for the toast to be read,
  // otherwise the failure would be invisible.
  const handleLanguageChange = async (next: SupportedLocale) => {
    if (next === currentLocale) return;
    localeAdapter.persist(next);

    let syncFailed = false;
    if (user) {
      try {
        await api.updateMe({ language: next });
      } catch {
        syncFailed = true;
      }
    }

    if (syncFailed) {
      toast.warning(t(($) => $.preferences.language.sync_failed));
      // Give the toast 2.5s of visible time before navigating away.
      setTimeout(() => window.location.reload(), 2500);
      return;
    }
    toast.success(t(($) => $.auto_save.toast_saved), {
      id: "settings-auto-save",
    });
    // Keep the confirmation visible before the locale reload replaces the UI.
    setTimeout(() => window.location.reload(), 900);
  };

  return (
    <SettingsGroup title={t(($) => $.preferences.general_title)}>
      <SettingsFormRow
        label={t(($) => $.preferences.theme.title)}
        description={t(($) => $.preferences.theme.description)}
        minWidth={COMPACT_SELECT_MIN_WIDTH}
      >
        <SettingsSelect
          className="w-full"
          id="profile-theme"
          label={t(($) => $.preferences.theme.title)}
          options={themeOptions}
          value={currentTheme}
          onValueChange={(next) => {
            if (next === currentTheme) return;
            setTheme(next as (typeof themeOptions)[number]["value"]);
            toast.success(t(($) => $.auto_save.toast_saved), {
              id: "settings-auto-save",
            });
          }}
        />
      </SettingsFormRow>

      <SettingsFormRow
        label={t(($) => $.preferences.language.title)}
        description={t(($) => $.preferences.language.description)}
        minWidth={COMPACT_SELECT_MIN_WIDTH}
      >
        <SettingsSelect
          className="w-full"
          id="profile-language"
          label={t(($) => $.preferences.language.title)}
          options={languageOptions}
          value={currentLocale}
          onValueChange={(next) => {
            void handleLanguageChange(next as SupportedLocale);
          }}
        />
      </SettingsFormRow>

      <TimezoneRow />

      <StickyCommentBarRow />
    </SettingsGroup>
  );
}

function StickyCommentBarRow() {
  const { t } = useT("settings");
  const sticky = useCommentComposerStore((s) => s.sticky);
  const toggleSticky = useCommentComposerStore((s) => s.toggleSticky);
  const label = t(($) => $.preferences.sticky_comment_bar.title);

  return (
    <SettingsFormRow
      label={label}
      description={t(($) => $.preferences.sticky_comment_bar.hint)}
    >
      <SettingsSwitch
        id="profile-sticky-comment-bar"
        label={label}
        checked={sticky}
        onCheckedChange={() => {
          toggleSticky();
          toast.success(t(($) => $.auto_save.toast_saved), {
            id: "settings-auto-save",
          });
        }}
      />
    </SettingsFormRow>
  );
}

// The "follow the browser" option is not a timezone, and its wire payload is
// the empty string the backend translates to NULL. An empty option *value*
// would be indistinguishable from "nothing selected" to the select itself, so
// the state gets its own sentinel and the translation happens at the wire
// boundary.
const BROWSER_TZ_VALUE = "__browser__";

function TimezoneRow() {
  const { t } = useT("settings");
  const user = useAuthStore((s) => s.user);
  const setUser = useAuthStore((s) => s.setUser);
  const stored = user?.timezone ?? null;
  const browser = browserTimezone();
  const value = stored ?? BROWSER_TZ_VALUE;

  // Full IANA list (from timezoneOptions in common/timezone-select) so a
  // user needing a non-curated zone isn't stuck with ~18 common ones.
  // Memoized — timezoneOptions walks the whole IANA set per call:
  // `Intl.supportedValuesOf("timeZone")` measures 418 in the Electron renderer's
  // Chromium (this comment used to say "~600"), and the helper unions that with
  // the curated fallback, the current zone and the browser's. Measured on this
  // select: **421 rows** — and only 9 of them in the DOM at a time, because antd
  // v6 keeps the option list in a plain scrolling holder
  // (`.ant-select-dropdown-list-holder`) and draws a window into it. The list
  // length is that holder's `scrollHeight / rowHeight`; counting
  // `.ant-select-item-option` nodes measures the window. An earlier version of
  // this comment said "418-420 rows", which was the IANA set rather than the
  // list this select actually holds.
  const options = useMemo(
    () => timezoneOptions(stored ?? browser),
    [stored, browser],
  );

  const handleChange = async (next: string) => {
    if (next === value) return;
    const payload = next === BROWSER_TZ_VALUE ? "" : next;
    try {
      const updated = await api.updateMe({ timezone: payload });
      setUser(updated);
      toast.success(t(($) => $.auto_save.toast_saved), {
        id: "settings-auto-save",
      });
    } catch (err) {
      toast.error(
        err instanceof Error && err.message
          ? err.message
          : t(($) => $.preferences.timezone.sync_failed),
      );
    }
  };

  // The IANA list is long and mostly punctuation; the monospace face is what
  // makes "Asia/Shanghai" scannable against its neighbours. It reaches the
  // trigger and each option as inline style rather than a class, because antd
  // sets `font-size` on both elements itself and its selectors outrank a
  // utility — the full reasoning is in `settings-select.tsx`. The values are
  // still the tokens, so this is the design scale and not a pixel count.
  //
  // It has to be a style on each option rather than a wrapper node around each
  // option's text, because an option's label has to stay a plain string for the
  // select to give it a `title` (see `settings-select.tsx`), and the old
  // per-option `SelectItem className` is gone for the same reason.
  const TZ_TYPOGRAPHY: CSSProperties = {
    fontSize: "var(--text-caption)",
    lineHeight: "var(--text-caption--line-height)",
    fontFamily: "var(--font-mono)",
  };
  const formatTZLabel = (tz: string) =>
    tz === BROWSER_TZ_VALUE
      ? `${browser}${t(($) => $.preferences.timezone.browser_suffix)}`
      : tz;

  return (
    <SettingsFormRow
      label={t(($) => $.preferences.timezone.title)}
      description={t(($) => $.preferences.timezone.hint)}
      minWidth={WIDE_SELECT_MIN_WIDTH}
    >
      <SettingsSelect
        className="w-full"
        id="preferences-timezone"
        label={t(($) => $.preferences.timezone.title)}
        options={[
          { value: BROWSER_TZ_VALUE, label: formatTZLabel(BROWSER_TZ_VALUE) },
          ...options.map((timezone) => ({
            value: timezone,
            label: formatTZLabel(timezone),
          })),
        ]}
        // 421 rows on this select, measured; this is the list `settings-select.tsx`
        // names as the reason the wrapper chose the antd-backed `Select` over
        // the base-ui atoms in the first place. Filtering matches the value as
        // well as the label, so "Asia/Shanghai" and "Shanghai" both land —
        // measured: typing "shanghai" takes 421 rows to 1.
        search
        typography={TZ_TYPOGRAPHY}
        value={value}
        onValueChange={(next) => {
          void handleChange(next);
        }}
      />
    </SettingsFormRow>
  );
}
