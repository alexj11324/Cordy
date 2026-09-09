"use client";

import { useQuery } from "@tanstack/react-query";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { agentListOptions } from "@orvilo/core/workspace/queries";
import { runtimeListOptions } from "@orvilo/core/runtimes";
import type { AvatarSize } from "@orvilo/ui/lib/avatar-size";
import { AgentProviderAvatar } from "../agents/components/agent-provider-avatar";

/** One identity for assignment, conversation and activity surfaces: the same
 * Harness mark used by the Agent list and settings header. Query caches are
 * shared with the workspace directories; no avatar URLs need to be written. */
export function AgentIdentityAvatar({agentId, name, size, className}: {
  agentId: string;
  name?: string;
  size?: AvatarSize;
  className?: string;
}) {
  const workspaceId = useWorkspaceId();
  const {data: agents = []} = useQuery(agentListOptions(workspaceId));
  const {data: runtimes = []} = useQuery(runtimeListOptions(workspaceId));
  const agent = agents.find((item) => item.id === agentId);
  const runtime = runtimes.find((item) => item.id === agent?.runtime_id);
  return <AgentProviderAvatar unassigned={!agentId} provider={runtime?.provider} name={name ?? agent?.name ?? "Agent"} size={size ?? "sm"} className={className} />;
}
