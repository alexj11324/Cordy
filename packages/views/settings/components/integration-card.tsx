import type { ReactNode } from "react";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { IntegrationChannelIcon, type IntegrationChannel } from "./integration-channel-icon";

export type IntegrationCardProps = {
  action: ReactNode;
  channel: IntegrationChannel;
  description: string;
  iconClassName: string;
  status: ReactNode;
  title: string;
};

export function IntegrationCard({
  action,
  channel,
  description,
  iconClassName,
  status,
  title,
}: IntegrationCardProps) {
  return (
    <Card
      className="h-full border-surface-border/80 shadow-none transition-colors hover:border-surface-border"
      data-testid={`integration-channel-card-${channel}`}
    >
      <CardContent className="flex min-h-52 flex-col gap-5 p-5">
        <div className="flex items-start gap-4">
          <IntegrationChannelIcon
            channel={channel}
            size="lg"
            className={iconClassName}
          />
          <div className="min-w-0 flex-1">
            <h3 className="text-body font-semibold">{title}</h3>
            <p className="mt-1.5 text-caption leading-5 text-muted-foreground">
              {description}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">{status}</div>
        <div className="mt-auto flex min-h-9 items-center justify-end">
          {action}
        </div>
      </CardContent>
    </Card>
  );
}
