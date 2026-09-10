import type { ReactNode } from "react";
import { cn } from "@orvilo/ui/lib/utils";
import { IntegrationChannelIcon, type IntegrationChannel } from "./integration-channel-icon";

export type IntegrationCardProps = {
  action: ReactNode;
  channel: IntegrationChannel;
  iconClassName: string;
  status: ReactNode;
  title: string;
};

export function IntegrationCard({
  action,
  channel,
  iconClassName,
  status,
  title,
}: IntegrationCardProps) {
  return (
    <div
      data-slot="settings-section-card"
      data-testid={`integration-channel-card-${channel}`}
      className={cn(
        "flex h-full flex-col gap-4 rounded-xl border border-border bg-surface p-4",
      )}
    >
      <div className="flex items-center gap-4">
        <IntegrationChannelIcon
          channel={channel}
          size="lg"
          className={iconClassName}
        />
        <div className="min-w-0 flex-1">
          <h3 className="text-body font-semibold">{title}</h3>
        </div>
      </div>
      <div className="mt-auto flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">{status}</div>
        <div className="ml-auto flex min-h-9 max-w-full items-center justify-end">{action}</div>
      </div>
    </div>
  );
}
