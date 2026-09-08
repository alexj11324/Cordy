"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Brain, ExternalLink, Plus, Trash2 } from "lucide-react";
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
import { AppLink } from "../../navigation";
import { SlackMark } from "../../settings/components/slack-mark";
import { useT } from "../../i18n";
import { AutomationMemoryDialog } from "./automation-memory-dialog";
import { AutomationSlackToolRow } from "./automation-slack-tool-row";

export function AutomationToolsSection({
  automation,
  canWrite,
  onToolsChange,
  saving = false,
}: {
  automation: Pick<Automation, "id" | "tools">;
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
            {/* Paths from the user's mcp.svg; currentColor follows the theme. */}
            <svg viewBox="0 0 24 24" fill="currentColor" fillRule="evenodd" aria-hidden="true" className="size-4 shrink-0 text-muted-foreground">
              <path d="M15.688 2.343a2.588 2.588 0 00-3.61 0l-9.626 9.44a.863.863 0 01-1.203 0 .823.823 0 010-1.18l9.626-9.44a4.313 4.313 0 016.016 0 4.116 4.116 0 011.204 3.54 4.3 4.3 0 013.609 1.18l.05.05a4.115 4.115 0 010 5.9l-8.706 8.537a.274.274 0 000 .393l1.788 1.754a.823.823 0 010 1.18.863.863 0 01-1.203 0l-1.788-1.753a1.92 1.92 0 010-2.754l8.706-8.538a2.47 2.47 0 000-3.54l-.05-.049a2.588 2.588 0 00-3.607-.003l-7.172 7.034-.002.002-.098.097a.863.863 0 01-1.204 0 .823.823 0 010-1.18l7.273-7.133a2.47 2.47 0 00-.003-3.537z" />
              <path d="M14.485 4.703a.823.823 0 000-1.18.863.863 0 00-1.204 0l-7.119 6.982a4.115 4.115 0 000 5.9 4.314 4.314 0 006.016 0l7.12-6.982a.823.823 0 000-1.18.863.863 0 00-1.204 0l-7.119 6.982a2.588 2.588 0 01-3.61 0 2.47 2.47 0 010-3.54l7.12-6.982z" />
            </svg>
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
            <PopoverContent align="start" className="w-72 p-3 space-y-2">
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
              <p className="text-caption text-muted-foreground">
                {t(($) => $.settings.tools_mcp_hint)}
              </p>
              {mcpQuery.isPending ? (
                <p role="status" className="text-caption text-muted-foreground">{t(($) => $.settings.tools_loading)}</p>
              ) : mcpQuery.isError ? (
                <div className="space-y-2">
                  <p role="status" className="text-caption text-destructive">{t(($) => $.settings.tools_load_failed)}</p>
                  <Button size="sm" variant="outline" onClick={() => void mcpQuery.refetch()}>{t(($) => $.page.retry)}</Button>
                </div>
              ) : servers.length === 0 ? (
                <p className="text-caption text-muted-foreground">
                  {t(($) => $.settings.tools_mcp_empty)}
                </p>
              ) : (
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
              )}
              <Button size="sm" variant="outline" nativeButton={false}
                render={<AppLink href={`${wsPaths.settings()}?tab=mcp`} />}>
                {t(($) => $.settings.tools_mcp_manage)}
                <ExternalLink className="size-3.5" />
              </Button>
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
