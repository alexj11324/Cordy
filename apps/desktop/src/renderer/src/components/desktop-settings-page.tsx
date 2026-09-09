import { useLayoutEffect } from "react";
import { Download, Server } from "lucide-react";
import { SettingsPage } from "@orvilo/views/settings";
import { useT } from "@orvilo/views/i18n";
import { getActiveTab, useTabStore } from "@/stores/tab-store";
import { DaemonSettingsTab } from "./daemon-settings-tab";
import { UpdatesSettingsTab } from "./updates-settings-tab";

export function DesktopSettingsPage({ onBack }: { onBack?: () => void }) {
  const { t } = useT("settings");
  const settingsTitle = t(($) => $.page.title);

  useLayoutEffect(() => {
    const previousTitle = document.title;
    const enforceSettingsTitle = () => {
      if (document.title !== settingsTitle) document.title = settingsTitle;
    };
    enforceSettingsTitle();
    const observer = new MutationObserver(enforceSettingsTitle);
    const titleElement =
      document.head.querySelector("title") ??
      document.head.appendChild(document.createElement("title"));
    observer.observe(titleElement, {
      childList: true,
      subtree: true,
      characterData: true,
    });

    return () => {
      observer.disconnect();
      document.title =
        getActiveTab(useTabStore.getState())?.title || previousTitle;
    };
  }, [settingsTitle]);

  return (
    <SettingsPage
      variant="standalone"
      onDismiss={onBack}
      extraAccountTabs={[
        {
          value: "daemon",
          label: "Daemon",
          icon: Server,
          content: <DaemonSettingsTab />,
        },
        {
          value: "updates",
          label: t(($) => $.desktop.tabs.updates),
          icon: Download,
          content: <UpdatesSettingsTab />,
        },
      ]}
    />
  );
}
