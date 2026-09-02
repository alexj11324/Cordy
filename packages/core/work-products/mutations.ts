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
    },
  });
}
