"use client";

import { Bot, Users } from "lucide-react";
import { cn } from "@orvilo/ui/lib/utils";
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@orvilo/ui/components/ui/avatar";
import {
  AVATAR_SIZE_PX,
  DEFAULT_AVATAR_SIZE,
  type AvatarSize,
} from "@orvilo/ui/lib/avatar-size";
import { parseAvatarEmoji } from "@orvilo/ui/lib/avatar-emoji";
import { OrviloIcon } from "./orvilo-icon";

interface ActorAvatarProps {
  name: string;
  initials: string;
  avatarUrl?: string | null;
  isAgent?: boolean;
  isSystem?: boolean;
  isTeam?: boolean;
  size?: AvatarSize;
  className?: string;
}

function ActorAvatar({
  name,
  initials,
  avatarUrl,
  isAgent,
  isSystem,
  isTeam,
  size = DEFAULT_AVATAR_SIZE,
  className,
}: ActorAvatarProps) {
  const px = AVATAR_SIZE_PX[size];
  const iconPx = Math.round(px * 0.75);
  const emoji = parseAvatarEmoji(avatarUrl);
  const avatarSize = px >= 40 ? "lg" : px <= 24 ? "sm" : "default";

  // Use the Atlas/ReUI Avatar primitive for every actor. Product-specific
  // identity rules stay in this adapter: emoji avatars are a fallback, while
  // image failures are handled by Base UI Avatar's fallback state.
  return (
    <Avatar
      aria-label={name}
      data-actor-type={isAgent ? "agent" : isTeam ? "team" : isSystem ? "system" : "member"}
      size={avatarSize}
      className={cn(
        "overflow-hidden bg-muted text-muted-foreground",
        className,
      )}
      style={{ width: px, height: px, fontSize: px * 0.45 }}
    >
      {avatarUrl && !emoji ? <AvatarImage alt={name} src={avatarUrl} /> : null}
      <AvatarFallback className="text-[inherit]">
        {emoji ? (
          <span
            role="img"
            aria-label={name}
            className="select-none leading-none"
            style={{ fontSize: px * 0.58 }}
          >
            {emoji}
          </span>
        ) : isSystem ? (
          <OrviloIcon noSpin style={{ width: iconPx, height: iconPx }} />
        ) : isAgent ? (
          <Bot aria-hidden="true" style={{ width: iconPx, height: iconPx }} />
        ) : isTeam ? (
          <Users aria-hidden="true" style={{ width: iconPx, height: iconPx }} />
        ) : (
          initials
        )}
      </AvatarFallback>
    </Avatar>
  );
}

export { ActorAvatar, type ActorAvatarProps };
