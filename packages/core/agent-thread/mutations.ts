import {
  useMutation,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { api } from "../api";
import { useWorkspaceId } from "../hooks";
import { chatKeys } from "../chat/queries";
import { agentThreadKeys } from "./queries";
import type {
  ContinueAgentThreadRequest,
  ContinueAgentThreadResponse,
} from "../types";

/**
 * Invalidate every cache identity affected by a continuation. The Agent
 * thread query is opened under a stable root/opener task id, while the
 * mutation is sent to the latest child task, so the workspace prefix is the
 * authoritative refresh boundary.
 */
export function invalidateAgentThreadContinuationQueries(
  queryClient: QueryClient,
  wsId: string,
  taskId: string,
  continuationTaskId?: string | null,
) {
  queryClient.invalidateQueries({ queryKey: agentThreadKeys.all(wsId) });
  queryClient.invalidateQueries({ queryKey: agentThreadKeys.task(wsId, taskId) });
  queryClient.invalidateQueries({ queryKey: chatKeys.taskMessages(taskId) });
  if (continuationTaskId) {
    queryClient.invalidateQueries({
      queryKey: chatKeys.taskMessages(continuationTaskId),
    });
  }
}

export function useContinueAgentThread() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation<
    ContinueAgentThreadResponse,
    Error,
    { taskId: string; request: ContinueAgentThreadRequest }
  >({
    mutationFn: ({ taskId, request }) => api.continueAgentThread(taskId, request),
    onSuccess: (response, { taskId }) =>
      invalidateAgentThreadContinuationQueries(
        queryClient,
        wsId,
        taskId,
        response.continuation_task_id,
      ),
  });
}
