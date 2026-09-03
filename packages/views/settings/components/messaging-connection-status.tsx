"use client";

import { CheckCircle2, CircleAlert, Loader2, Unplug } from "lucide-react";
import {
  messagingConnectionState,
  type MessagingConnectionSource,
} from "@patchbay/core/types";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { useT } from "../../i18n";

export function MessagingConnectionStatus({
  installation,
  compact = false,
}: {
  installation: MessagingConnectionSource;
  compact?: boolean;
}) {
  const { t } = useT("settings");
  const state = messagingConnectionState(installation);
  const labels = {
    connected: t(($) => $.page.connection_status.connected),
    connecting: t(($) => $.page.connection_status.connecting),
    disconnected: t(($) => $.page.connection_status.disconnected),
    degraded: t(($) => $.page.connection_status.degraded),
    error: t(($) => $.page.connection_status.error),
    unavailable: t(($) => $.page.connection_status.unavailable),
    paused: t(($) => $.page.connection_status.paused),
    experimental: t(($) => $.page.connection_status.experimental),
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
        <span className="text-micro text-muted-foreground">
          {t(($) => $.page.connection_status.label)}
        </span>
      )}
      <Badge
        className="h-auto min-h-5 max-w-full whitespace-normal text-left"
        variant={
          state === "error" || state === "degraded"
            ? "destructive"
            : "secondary"
        }
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
    </span>
  );
}
