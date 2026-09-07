import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { chatKeys } from "../chat/queries";
import { issueKeys } from "../issues/queries";
import { agentThreadKeys } from "./queries";

export function useSteerAgentThread(wsId: string, taskId: string) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (queuedTaskId: string) => {
      const receipt = await api.prioritizeAgentThreadTask(taskId, queuedTaskId);
      if (receipt.task_id !== queuedTaskId) {
        throw new Error("Agent thread prioritized a different task");
      }
      const cancelled = await api.cancelTaskById(receipt.active_task_id);
      if (cancelled.id !== receipt.active_task_id) {
        throw new Error("Agent thread active task was not interrupted");
      }
      return receipt;
    },
    onSettled: (receipt, _error, queuedTaskId) => {
      queryClient.invalidateQueries({ queryKey: agentThreadKeys.all(wsId) });
      queryClient.invalidateQueries({ queryKey: agentThreadKeys.detail(wsId, taskId) });
      queryClient.invalidateQueries({ queryKey: issueKeys.tasksAll() });
      queryClient.invalidateQueries({ queryKey: chatKeys.taskMessages(queuedTaskId) });
      if (receipt?.active_task_id) {
        queryClient.invalidateQueries({
          queryKey: chatKeys.taskMessages(receipt.active_task_id),
        });
      }
    },
  });
}
