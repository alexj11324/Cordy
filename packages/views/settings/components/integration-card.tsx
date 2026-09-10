import type { ReactNode } from "react";
import { Frame, FrameFooter, FramePanel } from "@orvilo/ui/components/reui/frame";
import { Item, ItemMedia } from "@orvilo/ui/components/ui/item";
import { IntegrationChannelIcon, type IntegrationChannel } from "./integration-channel-icon";

export type IntegrationCardProps = {
  action: ReactNode;
  channel: IntegrationChannel;
  iconClassName: string;
  status: ReactNode;
  title: string;
};

// ReUI settings-13: a platform panel with its connection controls in the frame footer.
// https://reui.io/preview/base/settings-13
export function IntegrationCard({
  action,
  channel,
  iconClassName,
  status,
  title,
}: IntegrationCardProps) {
  return (
    <Frame
      data-testid={`integration-channel-card-${channel}`}
      spacing="sm"
      className="h-full"
    >
      <FramePanel className="flex flex-1 flex-col gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <Item variant="muted" className="size-12 shrink-0 justify-center p-0">
            <ItemMedia variant="icon" className="size-auto">
              <IntegrationChannelIcon
                channel={channel}
                size="lg"
                className={iconClassName}
              />
            </ItemMedia>
          </Item>
          <div className="min-w-0 flex-1">
            <h3 className="text-body font-semibold">{title}</h3>
          </div>
        </div>
      </FramePanel>
      <FrameFooter className="flex flex-row flex-wrap items-center justify-between gap-3">
        <div className="flex min-h-8 max-w-full items-center">{action}</div>
        <div className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-2">{status}</div>
      </FrameFooter>
    </Frame>
  );
}
