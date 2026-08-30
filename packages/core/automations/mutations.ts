import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { automationKeys } from "./queries";
import { useWorkspaceId } from "../hooks";
import type {
  CreateAutomationRequest,
  UpdateAutomationRequest,
  ListAutomationsResponse,
  GetAutomationResponse,
  CreateAutomationTriggerRequest,
  UpdateAutomationTriggerRequest,
} from "../types";

export function useCreateAutomation() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: (data: CreateAutomationRequest) => api.createAutomation(data),
    onSuccess: (newAutomation) => {
      qc.setQueryData<ListAutomationsResponse>(automationKeys.list(wsId), (old) =>
        old && !old.automations.some((a) => a.id === newAutomation.id)
          ? { ...old, automations: [...old.automations, newAutomation], total: old.total + 1 }
          : old,
      );
    },
    onSettled: () => {
      qc.invalidateQueries({ queryKey: automationKeys.list(wsId) });
    },
  });
}

export function useUpdateAutomation() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ id, ...data }: { id: string } & UpdateAutomationRequest) =>
      api.updateAutomation(id, data),
    onMutate: ({ id, ...data }) => {
      qc.cancelQueries({ queryKey: automationKeys.list(wsId) });
      const prevList = qc.getQueryData<ListAutomationsResponse>(automationKeys.list(wsId));
      const prevDetail = qc.getQueryData<GetAutomationResponse>(automationKeys.detail(wsId, id));
      // Request shape (AutomationSubscriberInput) lacks `created_at`, so it's
      // not assignable to the response shape. onSettled invalidates the
      // detail query and refetches the authoritative server payload.
      const { subscribers: _omitSubs, ...optimistic } = data;
      qc.setQueryData<ListAutomationsResponse>(automationKeys.list(wsId), (old) =>
        old
          ? {
              ...old,
              automations: old.automations.map((a) =>
                a.id === id ? { ...a, ...optimistic } : a,
              ),
            }
          : old,
      );
      qc.setQueryData<GetAutomationResponse>(automationKeys.detail(wsId, id), (old) =>
        old ? { ...old, automation: { ...old.automation, ...optimistic } } : old,
      );
      return { prevList, prevDetail, id };
    },
    onError: (_err, _vars, ctx) => {
      if (ctx?.prevList) qc.setQueryData(automationKeys.list(wsId), ctx.prevList);
      if (ctx?.prevDetail) qc.setQueryData(automationKeys.detail(wsId, ctx.id), ctx.prevDetail);
    },
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, vars.id) });
      qc.invalidateQueries({ queryKey: automationKeys.list(wsId) });
    },
  });
}

export function useDeleteAutomation() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: (id: string) => api.deleteAutomation(id),
    onMutate: async (id) => {
      await qc.cancelQueries({ queryKey: automationKeys.list(wsId) });
      const prevList = qc.getQueryData<ListAutomationsResponse>(automationKeys.list(wsId));
      qc.setQueryData<ListAutomationsResponse>(automationKeys.list(wsId), (old) =>
        old ? { ...old, automations: old.automations.filter((a) => a.id !== id), total: old.total - 1 } : old,
      );
      qc.removeQueries({ queryKey: automationKeys.detail(wsId, id) });
      return { prevList };
    },
    onError: (_err, _id, ctx) => {
      if (ctx?.prevList) qc.setQueryData(automationKeys.list(wsId), ctx.prevList);
    },
    onSettled: () => {
      qc.invalidateQueries({ queryKey: automationKeys.list(wsId) });
    },
  });
}

export function useTriggerAutomation() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: (id: string) => api.triggerAutomation(id),
    onSettled: (_data, _err, id) => {
      qc.invalidateQueries({ queryKey: automationKeys.runs(wsId, id) });
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, id) });
    },
  });
}

export function useGrantAutomationAccess() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ automationId, userId }: { automationId: string; userId: string }) =>
      api.grantAutomationAccess(automationId, userId),
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, vars.automationId) });
    },
  });
}

export function useRevokeAutomationAccess() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ automationId, userId }: { automationId: string; userId: string }) =>
      api.revokeAutomationAccess(automationId, userId),
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, vars.automationId) });
    },
  });
}

export function useCreateAutomationTrigger() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ automationId, ...data }: { automationId: string } & CreateAutomationTriggerRequest) =>
      api.createAutomationTrigger(automationId, data),
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, vars.automationId) });
    },
  });
}

export function useUpdateAutomationTrigger() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ automationId, triggerId, ...data }: { automationId: string; triggerId: string } & UpdateAutomationTriggerRequest) =>
      api.updateAutomationTrigger(automationId, triggerId, data),
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, vars.automationId) });
    },
  });
}

export function useDeleteAutomationTrigger() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ automationId, triggerId }: { automationId: string; triggerId: string }) =>
      api.deleteAutomationTrigger(automationId, triggerId),
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, vars.automationId) });
    },
  });
}

export function useRotateAutomationTriggerWebhookToken() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ automationId, triggerId }: { automationId: string; triggerId: string }) =>
      api.rotateAutomationTriggerWebhookToken(automationId, triggerId),
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.detail(wsId, vars.automationId) });
    },
  });
}

// Replay re-dispatches a previously-recorded delivery. The server creates
// a new delivery row (with `replayed_from_delivery_id`) and synchronously
// kicks off a new automation run. We invalidate both deliveries and runs so
// the new delivery and any resulting run show up immediately.
export function useReplayAutomationDelivery() {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  return useMutation({
    mutationFn: ({ automationId, deliveryId }: { automationId: string; deliveryId: string }) =>
      api.replayAutomationDelivery(automationId, deliveryId),
    onSettled: (_data, _err, vars) => {
      qc.invalidateQueries({ queryKey: automationKeys.deliveries(wsId, vars.automationId) });
      qc.invalidateQueries({ queryKey: automationKeys.runs(wsId, vars.automationId) });
    },
  });
}
