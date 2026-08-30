import { queryOptions } from "@tanstack/react-query";
import { api, ApiError } from "../api";
import type { DependencyGraphResponse } from "../types";

export const dependencyGraphKeys = {
  all: (wsId: string) => ["dependency-graphs", wsId] as const,
  list: (wsId: string, projectId?: string) =>
    [...dependencyGraphKeys.all(wsId), "list", projectId ?? null] as const,
  detail: (wsId: string, issueId: string) =>
    [...dependencyGraphKeys.all(wsId), "detail", issueId] as const,
};

export function dependencyGraphsOptions(wsId: string, projectId?: string) {
  return queryOptions({
    queryKey: dependencyGraphKeys.list(wsId, projectId),
    queryFn: () =>
      api.listDependencyGraphs({
        ...(projectId ? { projectId } : {}),
        limit: 64,
      }),
    select: (response) => response.graphs,
    staleTime: 0,
  });
}

export function dependencyGraphOptions(wsId: string, issueId: string) {
  return queryOptions<DependencyGraphResponse | null>({
    queryKey: dependencyGraphKeys.detail(wsId, issueId),
    queryFn: async () => {
      try {
        return await api.getDependencyGraph(issueId);
      } catch (error) {
        // A normal issue is allowed to have no active graph. Preserve real
        // transport/auth/server failures for the detail surface to report.
        if (error instanceof ApiError && error.status === 404) return null;
        throw error;
      }
    },
    staleTime: 0,
  });
}
