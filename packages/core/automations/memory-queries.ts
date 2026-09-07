import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const automationMemoryKeys = {
  all: (wsId: string, automationId: string) =>
    ["automations", wsId, "detail", automationId, "memories"] as const,
  list: (wsId: string, automationId: string) =>
    [...automationMemoryKeys.all(wsId, automationId), "list"] as const,
  detail: (wsId: string, automationId: string, name: string) =>
    [...automationMemoryKeys.all(wsId, automationId), "detail", name] as const,
};

export function automationMemoryListOptions(wsId: string, automationId: string) {
  return queryOptions({
    queryKey: automationMemoryKeys.list(wsId, automationId),
    queryFn: () => api.listAutomationMemories(automationId),
    enabled: wsId.length > 0 && automationId.length > 0,
    staleTime: 0,
    refetchOnMount: "always",
  });
}

export function automationMemoryOptions(
  wsId: string,
  automationId: string,
  name: string,
  options?: { enabled?: boolean },
) {
  return queryOptions({
    queryKey: automationMemoryKeys.detail(wsId, automationId, name),
    queryFn: () => api.getAutomationMemory(automationId, name),
    enabled: (options?.enabled ?? true) && wsId.length > 0 && automationId.length > 0 && name.length > 0,
    staleTime: 0,
    refetchOnMount: "always",
  });
}
