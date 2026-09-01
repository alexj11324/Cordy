import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const automationKeys = {
  all: (wsId: string) => ["automations", wsId] as const,
  usage: (wsId: string) => [...automationKeys.all(wsId), "usage"] as const,
  list: (wsId: string) => [...automationKeys.all(wsId), "list"] as const,
  detail: (wsId: string, id: string) =>
    [...automationKeys.all(wsId), "detail", id] as const,
  runs: (wsId: string, id: string) =>
    [...automationKeys.all(wsId), "runs", id] as const,
  run: (wsId: string, automationId: string, runId: string) =>
    [...automationKeys.all(wsId), "runs", automationId, runId] as const,
  deliveries: (wsId: string, id: string) =>
    [...automationKeys.all(wsId), "deliveries", id] as const,
  delivery: (wsId: string, automationId: string, deliveryId: string) =>
    [...automationKeys.all(wsId), "deliveries", automationId, deliveryId] as const,
  cronPreview: (wsId: string, expr: string, tz: string) =>
    [...automationKeys.all(wsId), "cron-preview", expr, tz] as const,
};

export function automationQuotaUsageOptions(wsId: string) {
  return queryOptions({
    queryKey: automationKeys.usage(wsId),
    queryFn: () => api.getAutomationQuotaUsage(),
    enabled: wsId.length > 0,
    staleTime: 30_000,
    refetchOnWindowFocus: true,
  });
}

export function automationListOptions(wsId: string) {
  return queryOptions({
    queryKey: automationKeys.list(wsId),
    queryFn: () => api.listAutomations(),
    select: (data) => data.automations,
  });
}

export function automationDetailOptions(wsId: string, id: string) {
  return queryOptions({
    queryKey: automationKeys.detail(wsId, id),
    queryFn: () => api.getAutomation(id),
  });
}

export function automationRunsOptions(wsId: string, id: string) {
  return queryOptions({
    queryKey: automationKeys.runs(wsId, id),
    queryFn: () => api.listAutomationRuns(id),
    select: (data) => data.runs,
  });
}

// automationRunOptions fetches a single run with its full trigger_payload.
// The list endpoint (automationRunsOptions) omits trigger_payload to keep
// list responses small; callers (e.g. the run-detail dialog) use this
// query on demand when the user opens a run.
export function automationRunOptions(
  wsId: string,
  automationId: string,
  runId: string,
  options?: { enabled?: boolean },
) {
  return queryOptions({
    queryKey: automationKeys.run(wsId, automationId, runId),
    queryFn: () => api.getAutomationRun(automationId, runId),
    enabled: options?.enabled ?? true,
  });
}

// automationDeliveriesOptions powers the Deliveries section in the automation
// detail page. The list is slim — raw_body / selected_headers / response_body
// are omitted server-side. Detail rows are fetched on-demand when the user
// expands a row (see automationDeliveryOptions).
export function automationDeliveriesOptions(
  wsId: string,
  automationId: string,
  options?: { enabled?: boolean },
) {
  return queryOptions({
    queryKey: automationKeys.deliveries(wsId, automationId),
    queryFn: () => api.listAutomationDeliveries(automationId),
    select: (data) => data.deliveries,
    enabled: options?.enabled ?? true,
  });
}

// automationDeliveryOptions fetches the full delivery row including raw_body
// and headers subset. Used by the detail dialog opened from a list row.
export function automationDeliveryOptions(
  wsId: string,
  automationId: string,
  deliveryId: string,
  options?: { enabled?: boolean },
) {
  return queryOptions({
    queryKey: automationKeys.delivery(wsId, automationId, deliveryId),
    queryFn: () => api.getAutomationDelivery(automationId, deliveryId),
    enabled: options?.enabled ?? true,
  });
}

// cronPreviewOptions backs the schedule editor's next-run preview. The server
// owns cron/timezone evaluation, so the editor never approximates it locally.
export function cronPreviewOptions(
  wsId: string,
  expr: string,
  tz: string,
  options?: { enabled?: boolean },
) {
  return queryOptions({
    queryKey: automationKeys.cronPreview(wsId, expr, tz),
    queryFn: () => api.cronPreview({ expr, tz }),
    enabled: options?.enabled ?? true,
    staleTime: 30_000,
    // A 400 (invalid expression/timezone) is a stable answer for this input,
    // not a transient failure — retrying would only delay the inline error.
    retry: false,
  });
}
