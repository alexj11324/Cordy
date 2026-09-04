import { useLayoutEffect } from "react";
import { ArrowLeft, Download, Server } from "lucide-react";
import { SettingsPage } from "@patchbay/views/settings";
import { useT } from "@patchbay/views/i18n";
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
      navigationHeader={
        onBack ? (
          <button
            type="button"
            onClick={onBack}
            data-settings-initial-focus
            className="mt-12 mb-1 inline-flex h-8 items-center gap-2 rounded-lg px-2 text-body font-medium text-sidebar-text-secondary transition-colors hover:bg-sidebar-item-hover hover:text-sidebar-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
          >
            <ArrowLeft className="size-4" />
            {t(($) => $.page.back_to_app)}
          </button>
        ) : undefined
      }
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
