import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { useWorkspaceId } from "../hooks";
import type {
  CreateWorkProductRelationRequest,
  WorkProductRelation,
  WorkProductRelationPage,
} from "../types";
import { workProductKeys } from "./queries";

export function useCreateWorkProductRelation() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: ({
      issueId,
      ...body
    }: { issueId: string } & CreateWorkProductRelationRequest) =>
      api.createWorkProductRelation(issueId, body),
    onSuccess: (relation: WorkProductRelation) => {
      queryClient.setQueryData<WorkProductRelationPage>(
        workProductKeys.relations(wsId, relation.issue_id),
        (current) =>
          current && !current.relations.some((item) => item.id === relation.id)
            ? { ...current, relations: [relation, ...current.relations] }
            : current,
      );
    },
    onSettled: (_data, _error, variables) => {
      queryClient.invalidateQueries({
        queryKey: workProductKeys.relationsRoot(wsId, variables.issueId),
      });
      queryClient.invalidateQueries({
        queryKey: workProductKeys.provenanceRoot(wsId),
      });
      queryClient.invalidateQueries({
        queryKey: workProductKeys.issueProductsRoot(variables.issueId),
      });
    },
  });
}

/**
 * Detach retracts an attach. The server soft-closes the relation and keeps the
 * row, so there is nothing useful to write into the cache optimistically — the
 * lists are refetched instead, which also picks up the case where the server
 * refused the detach because the caller did not own the relation.
 */
export function useDetachWorkProductRelation() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: ({ issueId, relationId }: { issueId: string; relationId: string }) =>
      api.detachWorkProductRelation(issueId, relationId),
    onSettled: (_data, _error, variables) => {
      queryClient.invalidateQueries({
        queryKey: workProductKeys.issueProductsRoot(variables.issueId),
      });
      queryClient.invalidateQueries({
        queryKey: workProductKeys.relationsRoot(wsId, variables.issueId),
      });
    },
  });
}
