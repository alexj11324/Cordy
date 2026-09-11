"use client";

import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { EllipsisVerticalIcon } from "lucide-react";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu";
import { cn } from "@orvilo/ui/lib/utils";

export function ConnectionDotBadge({
  connected,
  children,
}: {
  connected: boolean;
  children: ReactNode;
}) {
  return (
    <Badge variant="outline">
      <span
        aria-hidden="true"
        className={cn("size-1.5 rounded-full", connected ? "bg-success" : "bg-warning")}
      />
      {children}
    </Badge>
  );
}

export type IntegrationRowMenuItem = {
  label: string;
  icon?: LucideIcon;
  onSelect: () => void;
  variant?: "destructive";
};

export function IntegrationRowMenu({
  ariaLabel,
  items,
}: {
  ariaLabel: string;
  items: readonly IntegrationRowMenuItem[];
}) {
  if (items.length === 0) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            aria-label={ariaLabel}
            size="icon-xs"
            type="button"
            variant="outline"
          >
            <EllipsisVerticalIcon aria-hidden="true" />
          </Button>
        }
      />
      <DropdownMenuContent align="end" className="min-w-44">
        <DropdownMenuGroup>
          {items.map((item) => (
            <DropdownMenuItem
              key={item.label}
              onClick={item.onSelect}
              variant={item.variant}
            >
              {item.icon ? <item.icon aria-hidden="true" /> : null}
              {item.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
