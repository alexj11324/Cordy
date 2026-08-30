import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import type { AttachIssuePullRequestRequest, AttachWorkProductRequest } from "../types";
import { githubKeys } from "./queries";

/** Creates the canonical explicit Issue -> Work Product relation. */
export function useAttachIssuePullRequest(issueId: string, wsId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: AttachIssuePullRequestRequest) =>
      api.attachIssuePullRequest(issueId, request),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: githubKeys.pullRequests(issueId) }),
        queryClient.invalidateQueries({ queryKey: githubKeys.workProducts(issueId) }),
        queryClient.invalidateQueries({ queryKey: githubKeys.unassociatedWorkProducts(wsId) }),
      ]);
    },
  });
}

/** Explicitly attaches an already mirrored, unassociated Work Product. */
export function useAttachIssueWorkProduct(issueId: string, wsId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: AttachWorkProductRequest) =>
      api.attachIssueWorkProduct(issueId, request),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: githubKeys.pullRequests(issueId) }),
        queryClient.invalidateQueries({ queryKey: githubKeys.workProducts(issueId) }),
        queryClient.invalidateQueries({ queryKey: githubKeys.unassociatedWorkProducts(wsId) }),
      ]);
    },
  });
}

/** Explicitly detaches a Work Product from the current issue. */
export function useDetachIssueWorkProduct(issueId: string, wsId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (workProductId: string) => api.detachIssueWorkProduct(issueId, workProductId),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: githubKeys.pullRequests(issueId) }),
        queryClient.invalidateQueries({ queryKey: githubKeys.workProducts(issueId) }),
        queryClient.invalidateQueries({ queryKey: githubKeys.unassociatedWorkProducts(wsId) }),
      ]);
    },
  });
}
