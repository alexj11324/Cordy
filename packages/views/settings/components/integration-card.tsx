/**
 * One integration channel row: identity, title, description, status, action.
 *
 * **This is the one file of Task 8b's three shared `integration-*` components
 * that has nothing to convert, and the reason is checkable rather than a
 * judgement call.** R88 converts the family, and the reference's mapping table
 * keeps its body: `ReUI Frame / PhoneInput / Item` → **keep** (decision 11).
 * The body here is exactly that `Item` — the ReUI settings-4 row this file's
 * own comment cites — plus `IntegrationChannelIcon`, which is a local SVG. No
 * Lobe primitive replaces either, so the conversion is a no-op: there is no
 * Lobe element to mount and this component needs no bridge of its own. What it
 * *hosts* changes instead — its `status` and `action` slots receive
 * `ConnectionDotBadge` (a shadcn `Badge`, also exempt) and `IntegrationRowMenu`
 * (Lobe since this task) — and those live on the callers' side of the boundary.
 *
 * R88's cost note anticipated `IntegrationCard` becoming a `Form.Group`. That
 * would be a redesign of the row, not a primitive swap, and it would take
 * `integrations-tab.test.tsx`'s `[data-slot=item-title]` assertions with it —
 * which R88's own table, listing exactly one red assertion (`ol > li` in the
 * setup guide), does not predict. Flagged in the task-8b report rather than
 * decided here.
 */

import type { ReactNode } from "react";
import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@orvilo/ui/components/ui/item";
import { IntegrationChannelIcon, type IntegrationChannel } from "./integration-channel-icon";

export type IntegrationCardProps = {
  action: ReactNode;
  channel: IntegrationChannel;
  description?: string;
  iconClassName?: string;
  status: ReactNode;
  title: string;
};

// ReUI settings-4 connection row.
// https://reui.io/preview/base/settings-4
export function IntegrationCard({
  action,
  channel,
  description,
  iconClassName,
  status,
  title,
}: IntegrationCardProps) {
  return (
    <Item
      data-testid={`integration-channel-card-${channel}`}
      size="sm"
      className="border-input border-0 border-b border-dashed px-0 py-3 last:border-b-0"
    >
      <ItemMedia className="translate-y-0! self-center!">
        <Item className="flex size-10 items-center justify-center border-2 border-background bg-muted/60 p-0 shadow-[0_1px_3px_0_rgba(0,0,0,0.14)] dark:border">
          <IntegrationChannelIcon
            channel={channel}
            size="sm"
            className={iconClassName}
          />
        </Item>
      </ItemMedia>
      <ItemContent>
        <ItemTitle>{title}</ItemTitle>
        {description ? <ItemDescription>{description}</ItemDescription> : null}
      </ItemContent>
      <ItemActions className="gap-2">
        {status}
        {action}
      </ItemActions>
    </Item>
  );
}
