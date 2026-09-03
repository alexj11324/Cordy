import { useMutation, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { chatKeys } from "../chat/queries";
import { issueKeys } from "../issues/queries";
import type { ContinueAgentThreadRequest } from "../types";
import { agentThreadKeys } from "./queries";

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
  queryClient.invalidateQueries({
    queryKey: agentThreadKeys.detail(wsId, taskId),
  });
  queryClient.invalidateQueries({ queryKey: issueKeys.tasksAll() });
  queryClient.invalidateQueries({
    queryKey: chatKeys.taskMessages(taskId),
  });
  if (continuationTaskId) {
    queryClient.invalidateQueries({
      queryKey: chatKeys.taskMessages(continuationTaskId),
    });
  }
}

export function useContinueAgentThread(wsId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ taskId, request }: { taskId: string; request: ContinueAgentThreadRequest }) =>
      api.continueAgentThread(taskId, request),
    onSuccess: (result, variables) =>
      invalidateAgentThreadContinuationQueries(
        queryClient,
        wsId,
        variables.taskId,
        result.continuation_task_id,
      ),
  });
}
