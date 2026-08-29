"use client";

import type { ReactNode } from "react";
import { ArrowUp, Hourglass } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@patchbay/ui/components/ui/popover";
import { cn } from "@patchbay/ui/lib/utils";
import { useT } from "../../i18n";

interface ChatFollowUpMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onWait: () => void;
  onSteer: () => void;
  children: ReactNode;
}

export function ChatFollowUpMenu({
  open,
  onOpenChange,
  onWait,
  onSteer,
  children,
}: ChatFollowUpMenuProps) {
  const { t } = useT("chat");

  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      {/*
        The send control keeps focus in the composer by cancelling pointerdown.
        That also blocks PopoverTrigger from opening, so this overlay is only
        an anchor — the parent opens the menu through `open`.
      */}
      <div className="relative inline-flex">
        {children}
        <PopoverTrigger
          tabIndex={-1}
          render={
            <span
              className="pointer-events-none absolute inset-0"
              aria-hidden="true"
            />
          }
        />
      </div>
      <PopoverContent
        align="end"
        side="top"
        sideOffset={8}
        className="w-72 gap-1 p-1.5"
      >
        <p className="px-2 pb-1 pt-1.5 text-caption text-muted-foreground">
          {t(($) => $.follow_up.title)}
        </p>
        <button
          type="button"
          className={cn(
            "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left",
            "hover:bg-accent focus-visible:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/20",
          )}
          onClick={() => {
            onOpenChange(false);
            onWait();
          }}
        >
          <Hourglass className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span className="min-w-0">
            <span className="block text-body font-medium">
              {t(($) => $.follow_up.wait)}
            </span>
            <span className="block text-caption text-muted-foreground">
              {t(($) => $.follow_up.wait_hint)}
            </span>
          </span>
        </button>
        <button
          type="button"
          className={cn(
            "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left",
            "hover:bg-accent focus-visible:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/20",
          )}
          onClick={() => {
            onOpenChange(false);
            onSteer();
          }}
        >
          <ArrowUp className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span className="min-w-0">
            <span className="block text-body font-medium">
              {t(($) => $.follow_up.steer)}
            </span>
            <span className="block text-caption text-muted-foreground">
              {t(($) => $.follow_up.steer_hint)}
            </span>
          </span>
        </button>
      </PopoverContent>
    </Popover>
  );
}
