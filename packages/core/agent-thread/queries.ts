import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const agentThreadKeys = {
  all: (wsId: string) => ["agent-threads", wsId] as const,
  detail: (wsId: string, taskId: string) =>
    [...agentThreadKeys.all(wsId), taskId] as const,
};

export function agentThreadOptions(wsId: string, taskId: string) {
  return queryOptions({
    queryKey: agentThreadKeys.detail(wsId, taskId),
    queryFn: () => api.getAgentThread(taskId),
    enabled: wsId.length > 0 && taskId.length > 0,
    staleTime: 5_000,
    refetchOnWindowFocus: true,
  });
}
