/**
 * Mobile mirror of the canonical Agent thread query contract. The envelope
 * includes the current task, the complete ordered provider-session chain,
 * public structured events, and the server's explicit availability decision.
 */
import { queryOptions } from "@tanstack/react-query";
import { api } from "@/data/api";

export const agentThreadKeys = {
  all: (wsId: string | null) => ["agent-threads", wsId] as const,
  task: (wsId: string | null, taskId: string) =>
    [...agentThreadKeys.all(wsId), taskId] as const,
};

export const agentThreadOptions = (
  wsId: string | null,
  taskId: string | null | undefined,
) =>
  queryOptions({
    queryKey: agentThreadKeys.task(wsId, taskId ?? ""),
    queryFn: ({ signal }) => api.getAgentThread(taskId!, { signal }),
    enabled: Boolean(wsId && taskId),
    staleTime: 5_000,
    refetchOnWindowFocus: true,
  });
