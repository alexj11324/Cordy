import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { useWorkspaceId } from "../hooks";
import type { AttachExistingWorkProductRequest, IssuePullRequestAttachRequest } from "../types";
import { workProductKeys } from "./queries";

function invalidateWorkProductSurfaces(
  queryClient: ReturnType<typeof useQueryClient>,
  wsId: string | null,
  issueId?: string,
) {
  queryClient.invalidateQueries({ queryKey: workProductKeys.all(wsId) });
  queryClient.invalidateQueries({ queryKey: ["work-products", "issue"] });
  if (issueId) {
    queryClient.invalidateQueries({ queryKey: workProductKeys.issueProductsRoot(issueId) });
  }
}

export function useAttachExistingWorkProduct() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: ({ issueId, ...body }: { issueId: string } & AttachExistingWorkProductRequest) =>
      api.attachExistingWorkProduct(issueId, body),
    onSettled: (_data, _error, variables) => {
      invalidateWorkProductSurfaces(queryClient, wsId, variables.issueId);
    },
  });
}

export function useAttachIssuePullRequest() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: ({ issueId, ...body }: { issueId: string } & IssuePullRequestAttachRequest) =>
      api.attachIssuePullRequest(issueId, body),
    onSettled: (_data, _error, variables) => {
      invalidateWorkProductSurfaces(queryClient, wsId, variables.issueId);
    },
  });
}

export function useDetachWorkProduct() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: ({ issueId, workProductId }: { issueId: string; workProductId: string }) =>
      api.detachWorkProduct(issueId, workProductId),
    onSettled: (_data, _error, variables) => {
      invalidateWorkProductSurfaces(queryClient, wsId, variables.issueId);
    },
  });
}
