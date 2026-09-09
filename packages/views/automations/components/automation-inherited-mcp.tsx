"use client";

import { useQuery } from "@tanstack/react-query";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import {
  isRuntimeUsableForUser,
  runtimeCapabilitiesOptions,
  runtimeListOptions,
} from "@orvilo/core/runtimes";
import { agentListOptions, teamListOptions } from "@orvilo/core/workspace/queries";
import { Button } from "@orvilo/ui/components/ui/button";
import { useT } from "../../i18n";
import type { AssigneeSelection } from "./pickers/agent-picker";

export function AutomationInheritedMcp({
  assignee,
  overriddenNames,
}: {
  assignee: AssigneeSelection | null;
  overriddenNames: readonly string[];
}) {
  const { t } = useT("automations");
  if (!assignee) {
    return <p className="text-caption text-muted-foreground">{t(($) => $.settings.tools_mcp_choose_agent)}</p>;
  }
  return <SelectedAgentMcp assignee={assignee} overriddenNames={overriddenNames} />;
}

function SelectedAgentMcp({
  assignee,
  overriddenNames,
}: {
  assignee: AssigneeSelection;
  overriddenNames: readonly string[];
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const currentUserId = useAuthStore((state) => state.user?.id ?? null);
  const agents = useQuery({ ...agentListOptions(wsId), enabled: !!wsId });
  const teams = useQuery({ ...teamListOptions(wsId), enabled: !!wsId && assignee.type === "team" });
  const runtimes = useQuery({ ...runtimeListOptions(wsId), enabled: !!wsId });
  const agentId = assignee.type === "agent"
    ? assignee.id
    : teams.data?.find((team) => team.id === assignee.id)?.leader_id;
  const agent = agents.data?.find((candidate) => candidate.id === agentId);
  const runtime = runtimes.data?.find((candidate) => candidate.id === agent?.runtime_id);
  const canRead = runtime != null && isRuntimeUsableForUser(runtime, currentUserId);
  const discoverable = canRead && runtime.runtime_mode === "local" && runtime.status === "online";
  const inventory = useQuery(runtimeCapabilitiesOptions(discoverable ? runtime.id : null));
  const catalogFailed = agents.isError || runtimes.isError || (assignee.type === "team" && teams.isError);
  const catalogPending = agents.isPending || runtimes.isPending || (assignee.type === "team" && teams.isPending);

  const retry = () => {
    if (agents.isError) void agents.refetch();
    if (runtimes.isError) void runtimes.refetch();
    if (teams.isError && assignee.type === "team") void teams.refetch();
    if (inventory.isError) void inventory.refetch();
  };
  let notice: string | null = null;
  if (catalogFailed) notice = t(($) => $.settings.tools_load_failed);
  else if (catalogPending) notice = t(($) => $.settings.tools_loading);
  else if (!agent) notice = t(($) => $.settings.tools_mcp_agent_unavailable);
  else if (!runtime) notice = t(($) => $.settings.tools_mcp_no_runtime);
  else if (!canRead) notice = t(($) => $.settings.tools_mcp_runtime_forbidden);
  else if (runtime.status !== "online") notice = t(($) => $.settings.tools_mcp_runtime_offline);
  else if (runtime.runtime_mode !== "local") notice = t(($) => $.settings.tools_mcp_runtime_unsupported);
  else if (inventory.isError) notice = t(($) => $.settings.tools_load_failed);
  else if (inventory.isPending) notice = t(($) => $.settings.tools_loading);
  else if (!inventory.data?.mcpSupported) notice = t(($) => $.settings.tools_mcp_runtime_unsupported);
  else if (inventory.data.mcpServers.length === 0) notice = t(($) => $.settings.tools_mcp_inherited_empty);

  return (
    <section className="space-y-2" data-testid="automation-inherited-mcp">
      <p className="text-caption font-medium">
        {agent
          ? t(($) => $.settings.tools_mcp_inherited_title, { name: agent.name })
          : t(($) => $.settings.tools_mcp_inherited_heading)}
      </p>
      {notice ? (
        <div className="space-y-1">
          <p role="status" className="text-caption text-muted-foreground">{notice}</p>
          {(catalogFailed || (discoverable && inventory.isError)) && (
            <Button size="sm" variant="outline" onClick={retry}>{t(($) => $.page.retry)}</Button>
          )}
        </div>
      ) : (
        <>
          <ul className="space-y-1.5">
            {[...(inventory.data?.mcpServers ?? [])]
              .sort((a, b) => Number(b.enabled) - Number(a.enabled) || a.name.localeCompare(b.name))
              .map((server) => (
              <li key={server.name} className="flex min-w-0 items-start justify-between gap-3">
                <span className={`min-w-0 break-words text-body${server.enabled ? "" : " text-muted-foreground"}`}>{server.name}</span>
                <span className="shrink-0 text-caption text-muted-foreground">
                  {overriddenNames.includes(server.name)
                    ? t(($) => $.settings.tools_mcp_inherited_overridden)
                    : server.enabled
                      ? t(($) => $.settings.tools_mcp_inherited_enabled)
                      : t(($) => $.settings.tools_mcp_inherited_disabled)}
                </span>
              </li>
            ))}
          </ul>
          <p className="text-caption text-muted-foreground">{t(($) => $.settings.tools_mcp_inherited_hint)}</p>
        </>
      )}
    </section>
  );
}
