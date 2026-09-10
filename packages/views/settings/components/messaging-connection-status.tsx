"use client";

import { CheckCircle2, CircleAlert, Loader2, Unplug } from "lucide-react";
import {
  messagingConnectionState,
  type MessagingConnectionSource,
} from "@orvilo/core/types";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { cn } from "@orvilo/ui/lib/utils";
import { useT } from "../../i18n";

function badgeVariant(
  state: ReturnType<typeof messagingConnectionState>,
): "success-light" | "warning-light" | "destructive-light" | "info-light" | "secondary" {
  if (state === "connected") return "success-light";
  if (state === "connecting") return "info-light";
  if (state === "error" || state === "degraded") return "destructive-light";
  if (state === "unavailable") return "secondary";
  return "warning-light";
}

export function MessagingConnectionStatus({
  installation,
  compact = false,
}: {
  installation: MessagingConnectionSource;
  compact?: boolean;
}) {
  const { t } = useT("settings");
  const state = messagingConnectionState(installation);
  const experimental = installation.setup?.experimental === true;
  const labels = {
    connected: t(($) => $.page.connection_status.connected),
    connecting: t(($) => $.page.connection_status.connecting),
    disconnected: t(($) => $.page.connection_status.disconnected),
    degraded: t(($) => $.page.connection_status.degraded),
    error: t(($) => $.page.connection_status.error),
    unavailable: t(($) => $.page.connection_status.unavailable),
    paused: t(($) => $.page.connection_status.paused),
  };
  const Icon =
    state === "connected"
      ? CheckCircle2
      : state === "connecting"
        ? Loader2
        : state === "disconnected"
          ? Unplug
          : CircleAlert;
  return (
    <span
      role="status"
      aria-label={t(($) => $.page.connection_status.label)}
      className="inline-flex min-w-0 flex-wrap items-center gap-2"
    >
      {!compact && (
        <span className="text-body text-muted-foreground">
          {t(($) => $.page.connection_status.label)}
        </span>
      )}
      {compact ? (
        <Badge variant="outline">
          <span
            aria-hidden="true"
            className={cn(
              "size-1.5 rounded-full",
              state === "connected" ? "bg-success" : "bg-warning",
            )}
          />
          {labels[state]}
        </Badge>
      ) : (
      <Badge
        radius="full"
        size="sm"
        variant={badgeVariant(state)}
      >
        <Icon
          aria-hidden="true"
          className={
            state === "connecting"
              ? "size-3 animate-spin motion-reduce:animate-none"
              : "size-3"
          }
        />
        {labels[state]}
      </Badge>
      )}
      {experimental ? (
        <Badge radius="full" size="sm" variant="outline">
          {t(($) => $.page.connection_status.experimental)}
        </Badge>
      ) : null}
    </span>
  );
}
