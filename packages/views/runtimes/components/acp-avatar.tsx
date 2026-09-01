"use client";

import { useQuery } from "@tanstack/react-query";
import type { ComponentType, ReactNode } from "react";
import {
  Antigravity,
  ClaudeCode,
  CodeBuddy,
  Codex,
  Copilot,
  Cursor,
  DeepSeek,
  Grok,
  HermesAgent,
  Huawei,
  Kimi,
  Kiro,
  Minimax,
  OpenClaw,
  OpenCode,
  Pi,
  Qoder,
  Qwen,
  Trae,
} from "@lobehub/icons";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { runtimeListOptions } from "@patchbay/core/runtimes/queries";
import type { Agent, AgentRuntime } from "@patchbay/core/types";
import { agentListOptions } from "@patchbay/core/workspace/queries";
import { ActorAvatar as ActorAvatarBase } from "@patchbay/ui/components/common/actor-avatar";
import {
  AVATAR_SIZE_PX,
  DEFAULT_AVATAR_SIZE,
  type AvatarSize,
} from "@patchbay/ui/lib/avatar-size";
import { cn } from "@patchbay/ui/lib/utils";
import { ProviderLogo } from "./provider-logo";
import {
  acpAvatarSource,
  resolveAgentProvider,
  type AcpLobehubIconId,
} from "./acp-avatar-map";

const EMPTY_AGENTS: Agent[] = [];
const EMPTY_RUNTIMES: AgentRuntime[] = [];

type LobehubAvatarIcon = {
  Avatar: ComponentType<{
    size: number;
    shape?: "circle" | "square";
    className?: string;
  }>;
};

const LOBEHUB_AVATAR_BY_ID: Record<AcpLobehubIconId, LobehubAvatarIcon> = {
  ClaudeCode,
  CodeBuddy,
  Codex,
  Copilot,
  OpenCode,
  Huawei,
  OpenClaw,
  HermesAgent,
  Pi,
  Cursor,
  Kimi,
  DeepSeek,
  Kiro,
  Antigravity,
  Qoder,
  Trae,
  Grok,
  Qwen,
  Minimax,
};

export type AcpAvatarShape = "circle" | "square";

function avatarPx(size: AvatarSize | number | undefined): number {
  if (typeof size === "number") return size;
  return AVATAR_SIZE_PX[size ?? DEFAULT_AVATAR_SIZE];
}

function closestAvatarSize(px: number): AvatarSize {
  let best: AvatarSize = DEFAULT_AVATAR_SIZE;
  let bestDelta = Number.POSITIVE_INFINITY;
  for (const [name, value] of Object.entries(AVATAR_SIZE_PX) as [
    AvatarSize,
    number,
  ][]) {
    const delta = Math.abs(value - px);
    if (delta < bestDelta) {
      best = name;
      bestDelta = delta;
    }
  }
  return best;
}

export function AcpAvatar({
  provider,
  size,
  shape = "circle",
  className,
  name,
}: {
  provider: string | null | undefined;
  size?: AvatarSize | number;
  shape?: AcpAvatarShape;
  className?: string;
  name?: string;
}) {
  const px = avatarPx(size);
  const source = acpAvatarSource(provider);
  const label = name?.trim() || provider || "agent";

  let face: ReactNode;
  if (source.kind === "lobehub") {
    const Icon = LOBEHUB_AVATAR_BY_ID[source.id];
    face = <Icon.Avatar size={px} shape={shape} className={className} />;
  } else if (source.kind === "fallback") {
    face = (
      <span
        className={cn(
          "inline-flex shrink-0 items-center justify-center overflow-hidden bg-muted",
          shape === "circle" && "rounded-full",
          className,
        )}
        style={{
          width: px,
          height: px,
          borderRadius: shape === "square" ? Math.floor(px * 0.1) : undefined,
        }}
      >
        <ProviderLogo provider={source.provider} className="h-[75%] w-[75%]" />
      </span>
    );
  } else {
    face = (
      <ActorAvatarBase
        name={label}
        initials=""
        isAgent
        size={typeof size === "number" ? closestAvatarSize(size) : size}
        className={className}
      />
    );
  }

  return (
    <span
      role="img"
      aria-label={label}
      data-testid="acp-avatar"
      data-provider={provider ?? ""}
      data-source={source.kind}
      data-shape={shape}
      className="inline-flex shrink-0"
    >
      {face}
    </span>
  );
}

/**
 * Agent face for list rows, chips, and hover cards. Looks up the bound
 * runtime's provider so switching Codex → Claude Code → Kiro (or any other
 * shipping runtime) changes the logo without a custom upload.
 */
export function AgentAcpAvatar({
  agentId,
  provider: providerOverride,
  size,
  shape = "circle",
  className,
  name,
}: {
  agentId?: string;
  provider?: string | null;
  size?: AvatarSize | number;
  shape?: AcpAvatarShape;
  className?: string;
  name?: string;
}) {
  const wsId = useWorkspaceId();
  const { data: agents = EMPTY_AGENTS } = useQuery(agentListOptions(wsId));
  const { data: runtimes = EMPTY_RUNTIMES } = useQuery(runtimeListOptions(wsId));
  const provider =
    providerOverride ?? resolveAgentProvider(agentId, agents, runtimes);
  const resolvedName =
    name ?? (agentId ? agents.find((agent) => agent.id === agentId)?.name : undefined);

  return (
    <AcpAvatar
      provider={provider}
      size={size}
      shape={shape}
      className={className}
      name={resolvedName}
    />
  );
}
