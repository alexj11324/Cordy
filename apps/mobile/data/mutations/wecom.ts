import { useMutation, useQueryClient } from "@tanstack/react-query";
import type {
  ListWecomInstallationsResponse,
  RegisterWecomBYORequest,
} from "@patchbay/core/types";
import { api } from "@/data/api";
import { useWorkspaceStore } from "@/data/workspace-store";
import { wecomKeys } from "@/data/queries/wecom";

export type RegisterWecomVariables = {
  agentId: string;
  body: RegisterWecomBYORequest;
};

export function useRegisterWecomBYO() {
  const qc = useQueryClient();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);

  return useMutation({
    mutationKey: ["registerWecomBYO", wsId] as const,
    mutationFn: ({ agentId, body }: RegisterWecomVariables) => {
      if (!wsId) throw new Error("Workspace is not selected");
      return api.registerWecomBYO(wsId, agentId, body);
    },
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: wecomKeys.installations(wsId) });
    },
  });
}

export function useDisconnectWecomInstallation() {
  const qc = useQueryClient();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);

  return useMutation({
    mutationKey: ["disconnectWecomInstallation", wsId] as const,
    mutationFn: (installationId: string) => {
      if (!wsId) throw new Error("Workspace is not selected");
      return api.deleteWecomInstallation(wsId, installationId);
    },
    onMutate: async (installationId) => {
      if (!wsId) return undefined;
      const key = wecomKeys.installations(wsId);
      await qc.cancelQueries({ queryKey: key });
      const previous = qc.getQueryData<ListWecomInstallationsResponse>(key);
      qc.setQueryData<ListWecomInstallationsResponse>(key, (old) =>
        old
          ? {
              ...old,
              installations: old.installations.filter(
                (installation) => installation.id !== installationId,
              ),
            }
          : old,
      );
      return { key, previous };
    },
    onError: (_error, _installationId, context) => {
      if (context?.previous !== undefined) {
        qc.setQueryData(context.key, context.previous);
      }
    },
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: wecomKeys.installations(wsId) });
    },
  });
}
