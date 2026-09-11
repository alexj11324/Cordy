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
