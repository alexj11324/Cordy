import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { useWorkspaceId } from "../hooks";
import { chatKeys } from "../chat/queries";
import { agentThreadKeys } from "./queries";
import type {
  ContinueAgentThreadRequest,
  ContinueAgentThreadResponse,
} from "../types";

export function useContinueAgentThread() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation<
    ContinueAgentThreadResponse,
    Error,
    { taskId: string; request: ContinueAgentThreadRequest }
  >({
    mutationFn: ({ taskId, request }) => api.continueAgentThread(taskId, request),
    onSuccess: (response, { taskId }) => {
      queryClient.invalidateQueries({
        queryKey: agentThreadKeys.task(wsId, taskId),
      });
      queryClient.invalidateQueries({
        queryKey: chatKeys.taskMessages(taskId),
      });
      if (response.continuation_task_id) {
        queryClient.invalidateQueries({
          queryKey: chatKeys.taskMessages(response.continuation_task_id),
        });
      }
    },
  });
}
