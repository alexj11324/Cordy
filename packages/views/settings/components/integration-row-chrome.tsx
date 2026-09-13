"use client";

import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { EllipsisVerticalIcon } from "lucide-react";
import { ActionIcon, DropdownMenu } from "@lobehub/ui/base-ui";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { cn } from "@orvilo/ui/lib/utils";

/**
 * The shared chrome for an integration row: the connection-status badge and the
 * overflow menu.
 *
 * Both are rendered by `IntegrationsTab` (settings) and by `linear-tab`, so the
 * conversion to Lobe is bounded by *both* host surfaces — and by the test files
 * that render them, which is the surface that appears in no screenshot.
 * `integrations-tab.test.tsx` and `integration-setup-guide.test.tsx` both moved
 * onto `renderWithI18n(..., { lobe: true })` with this change;
 * `linear-tab.test.tsx` had its `lobe` option turned on for the same reason.
 *
 * `Badge` stays shadcn: Lobe has no `Badge` at all, and `Tag` is a different
 * component rather than a rename of it. That exemption is recorded against the
 * `Progress` (decision 7) and ReUI `Frame` / `PhoneInput` / `Item` (decision 11)
 * ones, and it is why `ConnectionDotBadge` is unchanged below.
 */

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
  // The old trigger was a shadcn `Button variant="outline" size="icon-xs"` — a
  // **bordered 24px** icon button (`size-6`). Lobe's `ActionIcon` defaults to
  // `variant="borderless"`, which resolves to `type="text"` and would have
  // dropped the outline; `variant="outlined"` keeps it, and `size="small"` is
  // Lobe's 24px (`es/base-ui/controlSize.mjs`) so the box does not move either.
  // Both props are read from this site's baseline treatment, not applied as a
  // blanket rule — the tree has two translations for `outline` and neither is
  // safe to grep across.
  return (
    <DropdownMenu
      items={items.map((item) => ({
        danger: item.variant === "destructive",
        icon: item.icon,
        key: item.label,
        label: item.label,
        onClick: () => item.onSelect(),
      }))}
      placement="bottomRight"
    >
      <ActionIcon
        aria-label={ariaLabel}
        icon={EllipsisVerticalIcon}
        size="small"
        variant="outlined"
      />
    </DropdownMenu>
  );
}
