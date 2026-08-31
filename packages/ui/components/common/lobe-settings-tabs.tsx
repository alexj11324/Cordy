"use client";

import type { ReactNode } from "react";
import {
  TabsIndicator,
  TabsList,
  TabsPanel,
  TabsRoot,
  TabsTab,
} from "@lobehub/ui/es/base-ui/Tabs/atoms";
import { cn } from "@patchbay/ui/lib/utils";

export type LobeSettingsTab = {
  value: string;
  label: ReactNode;
  icon: ReactNode;
  content: ReactNode;
};

export type LobeSettingsTabGroup = {
  label?: ReactNode;
  tabs: LobeSettingsTab[];
};

export function LobeSettingsTabs({
  value,
  onValueChange,
  orientation,
  groups,
  header,
  className,
  listClassName,
  tabClassName,
  panelClassName,
  contentClassName,
  dataSettingsVariant,
}: {
  value: string;
  onValueChange: (value: string) => void;
  orientation: "horizontal" | "vertical";
  groups: LobeSettingsTabGroup[];
  header?: ReactNode;
  className?: string;
  listClassName?: string;
  tabClassName?: string;
  panelClassName?: string;
  contentClassName?: string;
  dataSettingsVariant?: string;
}) {
  const tabs = groups.flatMap((group) => group.tabs);

  return (
    <TabsRoot
      value={value}
      onValueChange={(next) => {
        if (next) onValueChange(next);
      }}
      orientation={orientation}
      variant="square"
      size="middle"
      data-settings-ui="lobe"
      data-settings-variant={dataSettingsVariant}
      className={cn("min-h-0", className)}
    >
      <TabsList
        variant="square"
        className={cn("shrink-0", listClassName)}
      >
        <TabsIndicator variant="square" className="hidden" />
        {header}
        {groups.map((group) => (
          <div key={group.tabs[0]?.value ?? "group"} className="contents">
            {group.label ? (
              <div className="hidden px-2 pb-1 pt-2 text-caption font-medium text-sidebar-text-secondary md:block">
                {group.label}
              </div>
            ) : null}
            {group.tabs.map((tab) => (
              <TabsTab
                key={tab.value}
                value={tab.value}
                className={cn(
                  "justify-start gap-2 !rounded-md !px-2 !text-body !font-medium !text-sidebar-text-secondary hover:!bg-sidebar-item-hover hover:!text-sidebar-item-active-foreground data-active:!bg-sidebar-item-active data-active:!text-sidebar-item-active-foreground",
                  tabClassName,
                )}
              >
                {tab.icon}
                {tab.label}
              </TabsTab>
            ))}
          </div>
        ))}
      </TabsList>
      {tabs.map((tab) => (
        <TabsPanel
          key={tab.value}
          value={tab.value}
          className={cn("min-w-0 flex-1 !p-0 !outline-none", panelClassName)}
        >
          <div data-settings-content className={contentClassName}>
            {tab.content}
          </div>
        </TabsPanel>
      ))}
    </TabsRoot>
  );
}
