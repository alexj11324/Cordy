import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const linearKeys = {
  all: (workspaceId: string) => ["linear", workspaceId] as const,
  connection: (workspaceId: string) => [...linearKeys.all(workspaceId), "connection"] as const,
};

export const linearConnectionOptions = (workspaceId: string) =>
  queryOptions({
    queryKey: linearKeys.connection(workspaceId),
    queryFn: () => api.getLinearConnection(workspaceId),
    enabled: Boolean(workspaceId),
  });
