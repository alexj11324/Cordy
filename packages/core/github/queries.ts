import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const githubKeys = {
  all: (wsId: string) => ["github", wsId] as const,
  installations: (wsId: string) => [...githubKeys.all(wsId), "installations"] as const,
  repositories: (wsId: string, installationId: string) =>
    [...githubKeys.all(wsId), "installations", installationId, "repositories"] as const,
  pullRequests: (issueId: string) => ["github", "pull-requests", issueId] as const,
  workProducts: (issueId: string) => ["work-products", "issue", issueId] as const,
  unassociatedWorkProducts: (wsId: string) => ["work-products", wsId, "unassociated"] as const,
};

export const githubInstallationsOptions = (wsId: string) =>
  queryOptions({
    queryKey: githubKeys.installations(wsId),
    queryFn: () => api.listGitHubInstallations(wsId),
    enabled: !!wsId,
  });

export const githubInstallationRepositoriesOptions = (
  wsId: string,
  installationId: string,
) =>
  infiniteQueryOptions({
    queryKey: githubKeys.repositories(wsId, installationId),
    queryFn: ({ pageParam }) =>
      api.listGitHubInstallationRepositories(wsId, installationId, {
        page: pageParam,
        per_page: 100,
      }),
    initialPageParam: 1,
    getNextPageParam: (lastPage) => lastPage.next_page ?? undefined,
    enabled: !!wsId && !!installationId,
  });

export const issuePullRequestsOptions = (issueId: string) =>
  queryOptions({
    queryKey: githubKeys.pullRequests(issueId),
    queryFn: () => api.listIssuePullRequests(issueId),
    enabled: !!issueId,
  });

export const issueWorkProductsOptions = (issueId: string) =>
  queryOptions({
    queryKey: githubKeys.workProducts(issueId),
    queryFn: () => api.listIssueWorkProducts(issueId),
    enabled: !!issueId,
  });

export const unassociatedWorkProductsOptions = (
  wsId: string,
  query: string,
  enabled: boolean,
) =>
  infiniteQueryOptions({
    queryKey: [...githubKeys.unassociatedWorkProducts(wsId), { query }] as const,
    queryFn: ({ pageParam }) =>
      api.listUnassociatedWorkProducts({ page: pageParam, per_page: 20, query }),
    initialPageParam: 1,
    getNextPageParam: (lastPage) => lastPage.next_page ?? undefined,
    enabled: enabled && !!wsId,
  });
