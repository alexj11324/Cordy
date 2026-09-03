"use client";

import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { dingtalkGroupRoutesOptions, useUpdateDingTalkGroupRoute } from "@patchbay/core/dingtalk";
import { agentListOptions } from "@patchbay/core/workspace/queries";
import type { DingTalkGroupRoute, DingTalkInstallation } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@patchbay/ui/components/ui/select";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { useT } from "../../i18n";

export function DingTalkGroupRoutes({
  workspaceId,
  installations,
  canManage,
}: {
  workspaceId: string;
  installations: DingTalkInstallation[];
  canManage: boolean;
}) {
  const { t } = useT("settings");
  const routesQuery = useQuery(dingtalkGroupRoutesOptions(workspaceId));
  const agentsQuery = useQuery({ ...agentListOptions(workspaceId), enabled: !!workspaceId });
  const updateRoute = useUpdateDingTalkGroupRoute(workspaceId);
  const activeInstallations = new Set(installations.filter((item) => item.status === "active").map((item) => item.id));
  const routes = (routesQuery.data?.routes ?? []).filter((route) =>
    route.id && route.workspace_id === workspaceId && activeInstallations.has(route.installation_id),
  );
  const agents = agentsQuery.data ?? [];
  // The list endpoint already selects kind=user, including product-defined
  // user agents. A system_key alone must not remove an otherwise valid target.
  const eligibleAgents = agents.filter((agent) => !agent.archived_at);
  const canSelect = agentsQuery.isSuccess && eligibleAgents.length > 0 && !updateRoute.isPending;

  async function reassign(route: DingTalkGroupRoute, agentId: string) {
    if (!canManage || !canSelect || route.agent_id === agentId) return;
    try {
      await updateRoute.mutateAsync({ routeId: route.id, agentId });
      toast.success(t(($) => $.dingtalk.group_routes_updated));
    } catch {
      toast.error(t(($) => $.dingtalk.group_routes_update_failed));
    }
  }

  return (
    <section className="space-y-4">
      <div className="space-y-1.5">
        <h2 className="text-body font-semibold">{t(($) => $.dingtalk.group_routes_title)}</h2>
        <p className="max-w-3xl text-caption leading-relaxed text-muted-foreground">
          {t(($) => $.dingtalk.group_routes_description)}
        </p>
      </div>
      <Card>
        <CardContent className="space-y-4">
          {routesQuery.isLoading ? (
            <div aria-busy="true" aria-label={t(($) => $.dingtalk.groups_loading)} className="space-y-3">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          ) : routesQuery.isError ? (
            <div role="alert" className="space-y-2">
              <p className="text-body font-medium">{t(($) => $.dingtalk.group_routes_error_title)}</p>
              <p className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_error_description)}</p>
              <Button variant="outline" size="sm" disabled={routesQuery.isFetching} onClick={() => void routesQuery.refetch()}>
                {t(($) => $.dingtalk.group_routes_retry)}
              </Button>
            </div>
          ) : routes.length === 0 ? (
            <div className="space-y-2">
              <p className="text-body font-medium">{t(($) => $.dingtalk.group_routes_empty_title)}</p>
              <p className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_empty_description)}</p>
            </div>
          ) : (
            <>
              {canManage && agentsQuery.isError ? (
                <div role="alert" className="space-y-2">
                  <p className="text-body font-medium">{t(($) => $.dingtalk.group_routes_agents_error_title)}</p>
                  <p className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_agents_error_description)}</p>
                  <Button variant="outline" size="sm" disabled={agentsQuery.isFetching} onClick={() => void agentsQuery.refetch()}>
                    {t(($) => $.dingtalk.group_routes_agents_retry)}
                  </Button>
                </div>
              ) : canManage && agentsQuery.isLoading ? (
                <p role="status" className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_agents_loading)}</p>
              ) : canManage && eligibleAgents.length === 0 ? (
                <div className="space-y-2">
                  <p className="text-body font-medium">{t(($) => $.dingtalk.group_routes_agents_empty_title)}</p>
                  <p className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_agents_empty_description)}</p>
                </div>
              ) : null}
              <ul className="divide-y divide-border/70">
                {routes.map((route) => {
                  const title = route.conversation_title || route.conversation_id;
                  const selectedLabel = agents.find((agent) => agent.id === route.agent_id)?.name
                    || t(($) => $.dingtalk.group_routes_unknown_agent);
                  return (
                    <li key={route.id} className="flex min-w-0 flex-col gap-3 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between">
                      <div className="min-w-0 flex-1 space-y-1">
                        <p className="truncate text-body font-medium" title={title}>{title}</p>
                        <p className="truncate font-mono text-micro text-muted-foreground" title={route.conversation_id}>
                          {route.conversation_id}
                        </p>
                      </div>
                      {canManage ? (
                        <Select
                          items={eligibleAgents.map((agent) => ({ value: agent.id, label: agent.name }))}
                          value={route.agent_id}
                          disabled={!canSelect}
                          onValueChange={(agentId) => { if (agentId) void reassign(route, agentId); }}
                        >
                          <SelectTrigger className="w-full shrink-0 sm:w-56" aria-label={t(($) => $.dingtalk.group_routes_agent_label, { group: title })}>
                            <SelectValue><span className="truncate">{selectedLabel}</span></SelectValue>
                          </SelectTrigger>
                          <SelectContent>
                            {eligibleAgents.map((agent) => <SelectItem key={agent.id} value={agent.id}>{agent.name}</SelectItem>)}
                          </SelectContent>
                        </Select>
                      ) : (
                        <p className="min-w-0 truncate text-caption text-muted-foreground sm:max-w-56">{selectedLabel}</p>
                      )}
                    </li>
                  );
                })}
              </ul>
            </>
          )}
        </CardContent>
      </Card>
    </section>
  );
}
