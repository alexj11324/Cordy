"use client";

import { AVATAR_SIZE_PX, type AvatarSize } from "@orvilo/ui/lib/avatar-size";
import {
  Avatar,
  AvatarBadge,
  AvatarFallback,
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
}: {
  provider?: string | null;
  name: string;
  size?: AvatarSize;
  online?: boolean;
  onlineLabel?: string;
  className?: string;
}) {
  const pixels = AVATAR_SIZE_PX[size];

  return (
    <Avatar
      aria-label={name}
      size={pixels >= 40 ? "lg" : pixels <= 24 ? "sm" : "default"}
      className={cn("bg-muted", className)}
      style={{ width: pixels, height: pixels }}
    >
      <AvatarFallback>
        <ProviderLogo provider={provider ?? ""} className="size-[55%]" />
      </AvatarFallback>
      {online ? (
        <AvatarBadge
          className="bg-success"
          aria-label={onlineLabel}
          aria-hidden={onlineLabel ? undefined : true}
        />
      ) : null}
    </Avatar>
  );
}
