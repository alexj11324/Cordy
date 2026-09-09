"use client";

import { AVATAR_SIZE_PX, type AvatarSize } from "@orvilo/ui/lib/avatar-size";
import {
  AvatarBadge,
} from "@orvilo/ui/components/ui/avatar";
import { cn } from "@orvilo/ui/lib/utils";
import { ProviderLogo } from "../../runtimes/components/provider-logo";

export function AgentProviderAvatar({
  provider,
  name,
  size = "lg",
  online = false,
  onlineLabel,
  className,
  unassigned = false,
}: {
  provider?: string | null;
  name: string;
  size?: AvatarSize;
  online?: boolean;
  onlineLabel?: string;
  className?: string;
  unassigned?: boolean;
}) {
  const pixels = AVATAR_SIZE_PX[size];

  return (
    <span
      aria-label={name}
      data-size={pixels >= 40 ? "lg" : pixels <= 24 ? "sm" : "default"}
      data-agent-identity={unassigned ? "unassigned" : "assigned"}
      className={cn("group/avatar relative inline-flex shrink-0 items-center justify-center", unassigned && "rounded-full bg-muted", className)}
      style={{ width: pixels, height: pixels }}
    >
      <ProviderLogo provider={provider ?? ""} className={unassigned ? "size-[75%]" : "size-full"} />
      {online ? (
        <AvatarBadge
          className="bg-success"
          aria-label={onlineLabel}
          aria-hidden={onlineLabel ? undefined : true}
        />
      ) : null}
    </span>
  );
}
