/** User continuation mutations for the shared Agent thread surface. */
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type {
  ContinueAgentThreadRequest,
  ContinueAgentThreadResponse,
} from "@patchbay/core/types";
import { api } from "@/data/api";
import { chatKeys } from "@/data/queries/chat";
import { agentThreadKeys } from "@/data/queries/agent-thread";
import { useWorkspaceStore } from "@/data/workspace-store";

export function useContinueAgentThread() {
  const qc = useQueryClient();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);

  return useMutation<
    ContinueAgentThreadResponse,
    Error,
    { taskId: string; request: ContinueAgentThreadRequest }
  >({
    mutationFn: ({ taskId, request }) => api.continueAgentThread(taskId, request),
    onSuccess: (response, { taskId }) => {
      qc.invalidateQueries({ queryKey: agentThreadKeys.task(wsId, taskId) });
      qc.invalidateQueries({ queryKey: chatKeys.taskMessages(taskId) });
      if (response.continuation_task_id) {
        qc.invalidateQueries({
          queryKey: chatKeys.taskMessages(response.continuation_task_id),
        });
      }
    },
  });
}
