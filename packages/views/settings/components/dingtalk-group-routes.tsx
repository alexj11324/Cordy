"use client";

import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
// One design system in this file: `DingTalkGroupRoutes` has exactly one host —
// `DingTalkTab` (`./dingtalk-tab`), which is rendered only from
// `./integrations-tab`'s channel dialog, and that component's two hosts
// (`settings-page.tsx`, `integrations/index.tsx`) both mount `LobeThemeBridge`.
// Nothing here is reachable from the agent detail page.
import { Button as LobeButton, Skeleton } from "@lobehub/ui/base-ui";
import { dingtalkGroupRoutesOptions, useUpdateDingTalkGroupRoute } from "@orvilo/core/dingtalk";
import { agentListOptions } from "@orvilo/core/workspace/queries";
import type { DingTalkGroupRoute, DingTalkInstallation } from "@orvilo/core/types";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSelect } from "./settings-select";
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
  const installedInstallations = new Set(installations.filter((item) => item.status === "installed").map((item) => item.id));
  const routes = (routesQuery.data?.routes ?? []).filter((route) =>
    route.id && route.workspace_id === workspaceId && installedInstallations.has(route.installation_id),
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
    /* The `<section><h2>` heading and the description above it are one group's
       header now — that is where a section heading goes, and the rows'
       `divider` is where the card's `divide-y` went. */
    <SettingsGroup
      variant="outlined"
      title={t(($) => $.dingtalk.group_routes_title)}
      description={
        <span className="block max-w-3xl text-caption leading-relaxed text-muted-foreground">
          {t(($) => $.dingtalk.group_routes_description)}
        </span>
      }
    >
      {routesQuery.isLoading ? (
        <div aria-busy="true" aria-label={t(($) => $.dingtalk.groups_loading)} className="space-y-3">
          <Skeleton height={40} width="100%" />
          <Skeleton height={40} width="100%" />
        </div>
      ) : routesQuery.isError ? (
        <div role="alert" className="space-y-2">
          <p className="text-body font-medium">{t(($) => $.dingtalk.group_routes_error_title)}</p>
          <p className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_error_description)}</p>
          <LobeButton disabled={routesQuery.isFetching} onClick={() => void routesQuery.refetch()}>
            {t(($) => $.dingtalk.group_routes_retry)}
          </LobeButton>
        </div>
      ) : routes.length === 0 ? (
        <SettingsEmptyState
          title={t(($) => $.dingtalk.group_routes_empty_title)}
          description={t(($) => $.dingtalk.group_routes_empty_description)}
        />
      ) : (
        <>
          {canManage && agentsQuery.isError ? (
            <div role="alert" className="space-y-2">
              <p className="text-body font-medium">{t(($) => $.dingtalk.group_routes_agents_error_title)}</p>
              <p className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_agents_error_description)}</p>
              <LobeButton disabled={agentsQuery.isFetching} onClick={() => void agentsQuery.refetch()}>
                {t(($) => $.dingtalk.group_routes_agents_retry)}
              </LobeButton>
            </div>
          ) : canManage && agentsQuery.isLoading ? (
            <p role="status" className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_agents_loading)}</p>
          ) : canManage && eligibleAgents.length === 0 ? (
            <div className="space-y-2">
              <p className="text-body font-medium">{t(($) => $.dingtalk.group_routes_agents_empty_title)}</p>
              <p className="text-caption text-muted-foreground">{t(($) => $.dingtalk.group_routes_agents_empty_description)}</p>
            </div>
          ) : null}
          {routes.map((route, index) => {
            const title = route.conversation_title || route.conversation_id;
            const assignedName = agents.find((agent) => agent.id === route.agent_id)?.name;
            const selectedLabel = assignedName || t(($) => $.dingtalk.group_routes_unknown_agent);
            const options = eligibleAgents.map((agent) => ({ value: agent.id, label: agent.name }));
            // antd renders an option's *label*, and falls back to the raw value
            // when the value is not among the options — so a route still
            // assigned to an agent that has since been archived would show its
            // UUID. The eligible list is the assignable set, not the displayable
            // one; the current value is added back so the trigger keeps showing
            // the name. Re-picking it is a no-op (`reassign` returns early on an
            // unchanged id), so this widens nothing. The shadcn `Select` this
            // replaced did not need it: it rendered `SelectValue`'s children
            // rather than looking the value up.
            const selectOptions = eligibleAgents.some((agent) => agent.id === route.agent_id)
              ? options
              : [...options, { value: route.agent_id, label: selectedLabel }];
            return (
              <SettingsFormRow
                key={route.id}
                divider={index > 0}
                label={
                  <span className="block min-w-0 space-y-1">
                    <span className="block truncate text-body font-medium" title={title}>{title}</span>
                    <span
                      className="block truncate font-mono text-micro text-muted-foreground"
                      title={route.conversation_id}
                    >
                      {route.conversation_id}
                    </span>
                  </span>
                }
              >
                {canManage ? (
                  <SettingsSelect
                    className="w-full shrink-0 sm:w-56"
                    disabled={!canSelect}
                    label={t(($) => $.dingtalk.group_routes_agent_label, { group: title })}
                    onValueChange={(agentId) => { if (agentId) void reassign(route, agentId); }}
                    options={selectOptions}
                    value={route.agent_id}
                  />
                ) : (
                  <span className="block min-w-0 truncate text-caption text-muted-foreground sm:max-w-56">{selectedLabel}</span>
                )}
              </SettingsFormRow>
            );
          })}
        </>
      )}
    </SettingsGroup>
  );
}
