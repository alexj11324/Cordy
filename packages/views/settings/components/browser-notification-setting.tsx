"use client";

import { useEffect, useState } from "react";
import {
  getWebNotificationPermission,
  isWebNotificationSupported,
  requestWebNotificationPermission,
  type WebNotificationPermission,
} from "@orvilo/core/platform";
import { Button } from "@lobehub/ui/base-ui";
import { isDesktopShell } from "../../platform";
import { useT } from "../../i18n";
import { SettingsFormRow } from "./settings-shell";

/**
 * Web-only control for the browser permission that native notification banners
 * require. Desktop delivers banners through the OS via Electron (no browser
 * permission involved), so this renders nothing there. It also renders nothing
 * when the Notification API is unavailable (SSR, older browsers).
 *
 * Capability and permission are read from `window`, so the first paint defers
 * to a post-mount effect to keep SSR and client markup identical (no hydration
 * mismatch).
 *
 * It renders a bare `SettingsFormRow` and not a card of its own: it is mounted
 * inside the notifications tab's "System Notifications" group, and a nested
 * card would draw a second border inside the first.
 */
export function BrowserNotificationSetting() {
  const { t } = useT("settings");
  const [mounted, setMounted] = useState(false);
  const [permission, setPermission] =
    useState<WebNotificationPermission>("default");

  useEffect(() => {
    setMounted(true);
    setPermission(getWebNotificationPermission());
  }, []);

  // Pre-mount, on desktop, or where the API is missing → nothing to manage.
  if (!mounted || isDesktopShell() || !isWebNotificationSupported()) return null;

  const handleEnable = async () => {
    setPermission(await requestWebNotificationPermission());
  };

  const statusHint =
    permission === "granted"
      ? t(($) => $.notifications.browser.granted)
      : permission === "denied"
        ? t(($) => $.notifications.browser.denied)
        : t(($) => $.notifications.browser.hint);

  return (
    <SettingsFormRow
      label={t(($) => $.notifications.browser.label)}
      description={statusHint}
    >
      {permission === "default" && (
        <Button shape="round" type="primary" onClick={handleEnable}>
          {t(($) => $.notifications.browser.enable)}
        </Button>
      )}
      {permission === "granted" && (
        <span className="text-caption text-muted-foreground shrink-0 font-medium">
          {t(($) => $.notifications.browser.enabled_badge)}
        </span>
      )}
    </SettingsFormRow>
  );
}
