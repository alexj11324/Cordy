import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { useWorkspaceId } from "../hooks";
import type {
  AutomationMemoryFile,
  ListAutomationMemoriesResponse,
} from "../types";
import { automationMemoryKeys } from "./memory-queries";

export interface UpdateAutomationMemoryVariables {
  automationId: string;
  name: string;
  content: string;
  expected_revision: number;
}

export function useUpdateAutomationMemory() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: ({ automationId, name, content, expected_revision }: UpdateAutomationMemoryVariables) =>
      api.updateAutomationMemory(automationId, name, { content, expected_revision }),
    onSuccess: (file, variables) => {
      queryClient.setQueryData<AutomationMemoryFile>(
        automationMemoryKeys.detail(wsId, variables.automationId, variables.name),
        file,
      );
      queryClient.setQueryData<ListAutomationMemoriesResponse>(
        automationMemoryKeys.list(wsId, variables.automationId),
        (current) => ({
          items: [
            ...(current?.items ?? []).filter((item) => item.name !== file.name),
            { name: file.name, revision: file.revision, updated_at: file.updated_at },
          ].sort((left, right) => left.name.localeCompare(right.name)),
        }),
      );
    },
  });
}

export interface DeleteAutomationMemoryVariables {
  automationId: string;
  name: string;
  revision: number;
}

export function useDeleteAutomationMemory() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: ({ automationId, name, revision }: DeleteAutomationMemoryVariables) =>
      api.deleteAutomationMemory(automationId, name, revision),
    onSuccess: (_result, variables) => {
      queryClient.removeQueries({
        queryKey: automationMemoryKeys.detail(wsId, variables.automationId, variables.name),
      });
      queryClient.setQueryData<ListAutomationMemoriesResponse>(
        automationMemoryKeys.list(wsId, variables.automationId),
        (current) => ({
          items: (current?.items ?? []).filter((item) => item.name !== variables.name),
        }),
      );
    },
  });
}
