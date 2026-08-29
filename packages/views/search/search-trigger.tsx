"use client";

import { Search } from "lucide-react";
import { SidebarMenuButton } from "@patchbay/ui/components/ui/sidebar";
import {
  useShortcut,
} from "@patchbay/core/shortcuts";
import { useSearchStore } from "./search-store";
import { useT } from "../i18n";
import { ShortcutKeycaps } from "../common/shortcut-keycaps";

export function SearchTrigger() {
  const { t } = useT("search");
  const shortcut = useShortcut("openSearch");
  return (
    <SidebarMenuButton
      className="text-sidebar-text-secondary"
      onClick={() => useSearchStore.getState().setOpen(true)}
    >
      <Search className="text-sidebar-icon-secondary" />
      <span>{t(($) => $.trigger.label)}</span>
      {shortcut ? (
        <ShortcutKeycaps shortcut={shortcut} decorative className="pointer-events-none ml-auto" />
      ) : null}
    </SidebarMenuButton>
  );
}
