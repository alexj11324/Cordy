"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Brain, Plus, Trash2 } from "lucide-react";
import { parseAutomationTools, type AutomationToolsConfig } from "@orvilo/core/automations";
import { useUpdateAutomation } from "@orvilo/core/automations/mutations";
import { slackAutomationCatalogOptions, slackInstallationsOptions } from "@orvilo/core/slack/queries";
import { workspaceMcpServersOptions } from "@orvilo/core/workspace/queries";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { settingsPathForTriggerProvider } from "@orvilo/core/automations";
import type { Automation } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { Checkbox } from "@orvilo/ui/components/ui/checkbox";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@orvilo/ui/components/ui/popover";
import { toast } from "sonner";
import { SlackMark } from "../../settings/components/slack-mark";
import { McpMark } from "../../settings/components/mcp-mark";
import { useT } from "../../i18n";
import { AutomationMemoryDialog } from "./automation-memory-dialog";
import { AutomationSlackToolRow } from "./automation-slack-tool-row";
import { AutomationInheritedMcp } from "./automation-inherited-mcp";
import type { AssigneeSelection } from "./pickers/agent-picker";

export function AutomationToolsSection({
  automation,
  assignee = null,
  canWrite,
  onToolsChange,
  saving = false,
}: {
  automation: Pick<Automation, "id" | "tools">;
  assignee?: AssigneeSelection | null;
  onToolsChange?: (tools: AutomationToolsConfig) => void;
  saving?: boolean;
  canWrite: boolean;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const updateAutomation = useUpdateAutomation();
  const tools = useMemo(() => parseAutomationTools(automation.tools), [automation.tools]);
  const mcpQuery = useQuery(workspaceMcpServersOptions(wsId));
  const slack = useQuery(slackInstallationsOptions(wsId));
  const servers = mcpQuery.data ?? [];
  const slackInstallations = slack.data?.installations ?? [];
  const slackConnected = slackInstallations.some(
    (row) => (row.installation_status ?? row.status) === "installed",
  );
  const slackCatalog = useQuery(slackAutomationCatalogOptions(wsId, slackConnected));
  const installedSlackKey = slackInstallations
    .filter((row) => (row.installation_status ?? row.status) === "installed")
    .map((row) => row.id)
    .sort()
    .join(",");
  const catalogInstallationsRef = useRef<string | null>(null);
  useEffect(() => {
    if (!slackConnected) {
      catalogInstallationsRef.current = null;
      return;
    }
    if (catalogInstallationsRef.current === installedSlackKey) return;
    catalogInstallationsRef.current = installedSlackKey;
    void slackCatalog.refetch();
  }, [installedSlackKey, slackCatalog, slackConnected]);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [memoriesOpen, setMemoriesOpen] = useState(false);
  const busy = saving || updateAutomation.isPending;

  const persist = (next: AutomationToolsConfig) => {
    if (busy || !canWrite) return;
    if (onToolsChange) { onToolsChange(next); return; }
    updateAutomation.mutate(
      { id: automation.id, tools: { ...next } },
      {
        onSuccess: () => toast.success(t(($) => $.settings.toast_tools_updated)),
        onError: (err) => {
          toast.error(err instanceof Error ? err.message : t(($) => $.dialog.toast_update_failed));
        },
      },
    );
  };

  return (
    <section className="space-y-2" data-testid="automation-tools">
      <h2 className="text-caption font-medium uppercase tracking-wider text-muted-foreground">
        {t(($) => $.settings.section_tools)}
      </h2>
      <div className="space-y-1">
        {tools.memories && <div className="flex items-center gap-3 py-1.5">
          <Brain className="size-4 shrink-0 text-muted-foreground" />
          <p className="min-w-0 flex-1 text-body">{t(($) => $.settings.tools_memories)}</p>
          {canWrite && (
            <div className="flex items-center gap-1 shrink-0">
              <Button
                size="sm"
                variant="ghost"
                className="h-7 text-caption"
                disabled={!automation.id || busy}
                title={!automation.id ? t(($) => $.create_settings.memory_after_save) : undefined}
                onClick={() => setMemoriesOpen(true)}
              >
                {t(($) => $.settings.tools_manage)}
              </Button>
              <Button
                size="icon-sm"
                variant="ghost"
                className="text-muted-foreground hover:text-destructive"
                aria-label={t(($) => $.settings.tools_memories_remove)}
                disabled={busy}
                onClick={() => {
                  const { memories: _removed, ...next } = tools;
                  persist(next);
                }}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </div>
          )}
        </div>}

        {tools.slack_send && <AutomationSlackToolRow
          tools={tools}
          installations={slackInstallations}
          channels={slackCatalog.data?.channels ?? []}
          canWrite={canWrite}
          busy={busy}
          catalogPending={slack.isPending || (slackConnected && slackCatalog.isPending)}
          catalogError={slack.isError || (slackConnected && slackCatalog.isError)}
          connectHref={settingsPathForTriggerProvider(wsPaths.settings(), "slack")}
          onRetry={() => {
            if (slack.isError) void slack.refetch();
            if (slackCatalog.isError) void slackCatalog.refetch();
          }}
          onPersist={persist}
          onRemove={() => {
            const { slack_send: _removed, ...next } = tools;
            persist(next);
          }}
        />}

        {(tools.mcp_server_ids ?? []).map((id) => (
          <div key={id} className="flex items-center gap-3 py-1.5">
            <McpMark className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate text-body">
              {mcpQuery.isPending ? <Skeleton className="h-4 w-32" />
                : servers.find((server) => server.id === id)?.name
                  ?? (mcpQuery.isError ? t(($) => $.settings.tools_load_failed) : t(($) => $.settings.tools_mcp_unavailable))}
            </span>
            {canWrite && <Button size="icon-sm" variant="ghost" disabled={busy}
              className="text-muted-foreground hover:text-destructive"
              aria-label={t(($) => $.settings.tools_remove_mcp, { name: servers.find((server) => server.id === id)?.name ?? "MCP" })}
              onClick={() => persist({ ...tools, mcp_server_ids: tools.mcp_server_ids?.filter((selected) => selected !== id) })}>
              <Trash2 className="size-3.5" />
            </Button>}
          </div>
        ))}

        {canWrite && (
          <Popover open={mcpOpen} onOpenChange={setMcpOpen}>
            <PopoverTrigger
              render={
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-8 w-full justify-start px-0 font-normal text-muted-foreground hover:text-foreground"
                >
                  <Plus className="h-3.5 w-3.5" />
                  {t(($) => $.settings.tools_add)}
                </Button>
              }
            />
            <PopoverContent align="start" className="max-h-96 w-80 max-w-[calc(100vw-2rem)] space-y-2 overflow-y-auto p-3">
              {!tools.memories && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="w-full justify-start"
                  disabled={busy}
                  onClick={() => persist({ ...tools, memories: {} })}
                >
                  <Brain className="size-3.5" />
                  {t(($) => $.settings.tools_memories)}
                </Button>
              )}
              {!tools.slack_send && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="w-full justify-start"
                  disabled={busy}
                  onClick={() => persist({ ...tools, slack_send: {} })}
                >
                  <SlackMark className="size-3.5" />
                  {t(($) => $.settings.tools_slack)}
                </Button>
              )}
              {mcpOpen && (
                <AutomationInheritedMcp
                  assignee={assignee}
                  overriddenNames={servers
                    .filter((server) => !tools.mcp_server_ids || tools.mcp_server_ids.includes(server.id))
                    .map((server) => server.name)}
                />
              )}
              {mcpQuery.isPending ? (
                <p role="status" className="text-caption text-muted-foreground">{t(($) => $.settings.tools_loading)}</p>
              ) : mcpQuery.isError ? (
                <div className="space-y-2">
                  <p role="status" className="text-caption text-destructive">{t(($) => $.settings.tools_load_failed)}</p>
                  <Button size="sm" variant="outline" onClick={() => void mcpQuery.refetch()}>{t(($) => $.page.retry)}</Button>
                </div>
              ) : servers.length > 0 ? (
                <div className="space-y-2">
                  <p className="text-caption font-medium">
                    {t(($) => $.settings.tools_mcp_hint)}
                  </p>
                  <ul className="space-y-1.5">
                    {servers.map((server) => {
                      const selected = tools.mcp_server_ids?.includes(server.id) === true;
                      return (
                        <li key={server.id} className="flex items-center gap-2">
                          <Checkbox
                            aria-label={server.name}
                            disabled={busy}
                            checked={selected}
                            onCheckedChange={(checked) => {
                              const current = new Set(tools.mcp_server_ids ?? []);
                              if (checked === true) current.add(server.id);
                              else current.delete(server.id);
                              persist({ ...tools, mcp_server_ids: [...current] });
                            }}
                          />
                          <span className="text-body">{server.name}</span>
                        </li>
                      );
                    })}
                  </ul>
                </div>
              ) : null}
            </PopoverContent>
          </Popover>
        )}
      </div>
      {memoriesOpen && (
        <AutomationMemoryDialog
          automationId={automation.id}
          onOpenChange={setMemoriesOpen}
        />
      )}
    </section>
  );
}
