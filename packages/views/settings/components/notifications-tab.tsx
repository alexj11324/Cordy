"use client";

import { useQuery } from "@tanstack/react-query";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { notificationPreferenceOptions } from "@orvilo/core/notification-preferences/queries";
import { useUpdateNotificationPreferences } from "@orvilo/core/notification-preferences/mutations";
import type { NotificationGroupKey, NotificationPreferences } from "@orvilo/core/types";
import { toast } from "sonner";
import { useT } from "../../i18n";
import { BrowserNotificationSetting } from "./browser-notification-setting";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSwitch } from "./settings-switch";

// Inbox event groups rendered in the per-event toggle list. `system_notifications`
// is a sibling preference key but lives in its own section below.
const INBOX_GROUP_KEYS = [
  "assignments",
  "status_changes",
  "comments",
  "mentions",
  "updates",
  "agent_activity",
] as const;
type InboxGroupKey = (typeof INBOX_GROUP_KEYS)[number];

/**
 * Notification preferences. Every switch is immediate-effect — it PATCHes on
 * change and the switch's checked state is the mutation's own preference
 * object — so no row carries a `name` and the groups are layout only.
 *
 * Both groups used to be cards inside sections that carried their own titles;
 * they are now `SettingsGroup`s, which is the same thing with the title, the
 * card and the row chrome coming from one component. The page title and
 * description that `SettingsTab` used to render belong to the dialog header.
 */
export function NotificationsTab() {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const { data } = useQuery(notificationPreferenceOptions(wsId));
  const mutation = useUpdateNotificationPreferences();

  const preferences = data?.preferences ?? {};

  const handleToggle = (key: NotificationGroupKey, enabled: boolean) => {
    const updated: NotificationPreferences = {
      ...preferences,
      [key]: enabled ? "all" : "muted",
    };
    // Remove keys set to "all" (default) to keep the object clean
    if (enabled) {
      delete updated[key];
    }
    mutation.mutate(updated, {
      onSuccess: () =>
        toast.success(t(($) => $.auto_save.toast_saved), {
          id: "settings-auto-save",
        }),
      onError: (err) =>
        toast.error(
          err instanceof Error && err.message
            ? err.message
            : t(($) => $.notifications.toast_failed),
        ),
    });
  };

  const systemEnabled = preferences.system_notifications !== "muted";

  return (
    <>
      <SettingsGroup
        title={t(($) => $.notifications.title)}
        description={t(($) => $.notifications.description)}
      >
        {INBOX_GROUP_KEYS.map((key: InboxGroupKey) => {
          const enabled = preferences[key] !== "muted";
          const label = t(($) => $.notifications.groups[key].label);
          return (
            <SettingsFormRow
              key={key}
              label={label}
              description={t(($) => $.notifications.groups[key].description)}
            >
              <SettingsSwitch
                label={label}
                checked={enabled}
                onCheckedChange={(checked) => handleToggle(key, checked)}
              />
            </SettingsFormRow>
          );
        })}
      </SettingsGroup>

      <SettingsGroup
        title={t(($) => $.notifications.system.title)}
        description={t(($) => $.notifications.system.description)}
      >
        <SettingsFormRow
          label={t(($) => $.notifications.system.label)}
          description={t(($) => $.notifications.system.hint)}
        >
          <SettingsSwitch
            label={t(($) => $.notifications.system.label)}
            checked={systemEnabled}
            onCheckedChange={(checked) =>
              handleToggle("system_notifications", checked)
            }
          />
        </SettingsFormRow>

        {/* Web-only: the browser permission banners require. Renders nothing on
            desktop (OS-native delivery) or where the Notification API is absent. */}
        <BrowserNotificationSetting />
      </SettingsGroup>
    </>
  );
}
