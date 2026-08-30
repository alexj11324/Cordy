/** User continuation mutations for the shared Agent thread surface. */
import {
  useMutation,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import type {
  ContinueAgentThreadRequest,
  ContinueAgentThreadResponse,
} from "@patchbay/core/types";
import { api } from "@/data/api";
import { chatKeys } from "@/data/queries/chat";
import { agentThreadKeys } from "@/data/queries/agent-thread";
import { useWorkspaceStore } from "@/data/workspace-store";

export function invalidateAgentThreadContinuationQueries(
  qc: QueryClient,
  wsId: string | null,
  taskId: string,
  continuationTaskId?: string | null,
) {
  qc.invalidateQueries({ queryKey: agentThreadKeys.all(wsId) });
  qc.invalidateQueries({ queryKey: agentThreadKeys.task(wsId, taskId) });
  qc.invalidateQueries({ queryKey: chatKeys.taskMessages(taskId) });
  if (continuationTaskId) {
    qc.invalidateQueries({
      queryKey: chatKeys.taskMessages(continuationTaskId),
    });
  }
}

export function useContinueAgentThread() {
  const qc = useQueryClient();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);

  return useMutation<
    ContinueAgentThreadResponse,
    Error,
    { taskId: string; request: ContinueAgentThreadRequest }
  >({
    mutationFn: ({ taskId, request }) => api.continueAgentThread(taskId, request),
    onSuccess: (response, { taskId }) =>
      invalidateAgentThreadContinuationQueries(
        qc,
        wsId,
        taskId,
        response.continuation_task_id,
      ),
  });
}
