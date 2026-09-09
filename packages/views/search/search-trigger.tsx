"use client";

import { Button } from "@orvilo/ui/components/ui/button";
import { Kbd } from "@orvilo/ui/components/ui/kbd";
import { SearchIcon } from "lucide-react";
import { formatShortcut, useShortcut } from "@orvilo/core/shortcuts";
import { useSearchStore } from "./search-store";
import { useT } from "../i18n";

export function SearchTrigger() {
  const { t } = useT("search");
  const shortcut = useShortcut("openSearch");
  return (
    <div className="relative">
      <Button
        type="button"
        variant="outline"
        className="hover:bg-background h-8 w-full justify-start pl-7 font-normal transition-[width] duration-200 ease-linear in-data-[state=collapsed]:w-8! in-data-[state=collapsed]:pl-4! in-data-[state=collapsed]:text-transparent"
        onClick={() => useSearchStore.getState().setOpen(true)}
      >
        {t(($) => $.trigger.label)}
      </Button>
      <SearchIcon
        aria-hidden="true"
        className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 opacity-50 select-none"
      />
      <Kbd className="absolute top-1/2 right-2 -translate-y-1/2 in-data-[state=collapsed]:hidden">
        {formatShortcut(shortcut)}
      </Kbd>
    </div>
  );
}
