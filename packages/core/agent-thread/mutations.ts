import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { chatKeys } from "../chat/queries";
import { issueKeys } from "../issues/queries";
import type { ContinueAgentThreadRequest } from "../types";
import { agentThreadKeys } from "./queries";

export function useContinueAgentThread(wsId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ taskId, request }: { taskId: string; request: ContinueAgentThreadRequest }) =>
      api.continueAgentThread(taskId, request),
    onSuccess: (result, variables) => {
      queryClient.invalidateQueries({ queryKey: agentThreadKeys.all(wsId) });
      queryClient.invalidateQueries({
        queryKey: agentThreadKeys.detail(wsId, variables.taskId),
      });
      queryClient.invalidateQueries({ queryKey: issueKeys.tasksAll() });
      queryClient.invalidateQueries({
        queryKey: chatKeys.taskMessages(variables.taskId),
      });
      queryClient.invalidateQueries({
        queryKey: chatKeys.taskMessages(result.continuation_task_id),
      });
    },
  });
}
