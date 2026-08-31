import { ArrowLeft, Download, Server } from "lucide-react";
import { SettingsPage } from "@patchbay/views/settings";
import { useT } from "@patchbay/views/i18n";
import { DaemonSettingsTab } from "./daemon-settings-tab";
import { UpdatesSettingsTab } from "./updates-settings-tab";

export function DesktopSettingsPage({ onBack }: { onBack?: () => void }) {
  const { t } = useT("settings");

  return (
    <SettingsPage
      variant="standalone"
      navigationHeader={
        onBack ? (
          <button
            type="button"
            onClick={onBack}
            className="mb-5 mt-12 inline-flex h-8 items-center gap-2 rounded-md px-2 text-body font-medium text-sidebar-text-secondary transition-colors hover:bg-sidebar-item-hover hover:text-sidebar-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
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
