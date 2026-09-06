"use client";

import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Brain, ExternalLink, Plus, Trash2 } from "lucide-react";
import { parseAutomationTools, type AutomationToolsConfig } from "@patchbay/core/automations";
import { useUpdateAutomation } from "@patchbay/core/automations/mutations";
import { slackInstallationsOptions } from "@patchbay/core/slack/queries";
import { workspaceMcpServersOptions } from "@patchbay/core/workspace/queries";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { settingsPathForTriggerProvider } from "@patchbay/core/automations";
import type { Automation } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { Checkbox } from "@patchbay/ui/components/ui/checkbox";
import { Input } from "@patchbay/ui/components/ui/input";
import { Switch } from "@patchbay/ui/components/ui/switch";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@patchbay/ui/components/ui/popover";
import { toast } from "sonner";
import { AppLink } from "../../navigation";
import { SlackMark } from "../../settings/components/slack-mark";
import { useT } from "../../i18n";

export function AutomationToolsSection({
  automation,
  canWrite,
}: {
  automation: Automation;
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
  const slackConnected = (slack.data?.installations ?? []).some(
    (row) => row.status === "installed" || row.installation_status === "installed",
  );
  const [mcpOpen, setMcpOpen] = useState(false);
  const [memoriesOpen, setMemoriesOpen] = useState(false);

  const persist = (next: AutomationToolsConfig) => {
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

  const memoriesEnabled = tools.memories?.enabled === true;
  const slackEnabled = tools.slack_send?.enabled === true;

  return (
    <section className="space-y-2" data-testid="automation-tools">
      <h2 className="text-caption font-medium uppercase tracking-wider text-muted-foreground">
        {t(($) => $.settings.section_tools)}
      </h2>
      <div className="space-y-1">
        <div className="flex items-center gap-3 py-1.5">
          <Brain className="size-4 shrink-0 text-muted-foreground" />
          <p className="min-w-0 flex-1 text-body">{t(($) => $.settings.tools_memories)}</p>
          {canWrite && (
            <div className="flex items-center gap-1 shrink-0">
              <Popover open={memoriesOpen} onOpenChange={setMemoriesOpen}>
                <PopoverTrigger
                  render={
                    <Button size="sm" variant="ghost" className="h-7 text-caption">
                      {t(($) => $.settings.tools_manage)}
                    </Button>
                  }
                />
                <PopoverContent align="end" className="w-72 p-3 space-y-3">
                  <p className="text-caption text-muted-foreground">
                    {t(($) => $.settings.tools_memories_hint)}
                  </p>
                  <div className="flex items-center justify-between gap-3">
                    <span className="text-body">{t(($) => $.settings.tools_memories)}</span>
                    <Switch
                      size="sm"
                      checked={memoriesEnabled}
                      onCheckedChange={(enabled) => persist({ ...tools, memories: { enabled } })}
                    />
                  </div>
                </PopoverContent>
              </Popover>
              <Button
                size="icon-sm"
                variant="ghost"
                className="text-muted-foreground hover:text-destructive"
                aria-label={t(($) => $.settings.tools_memories_remove)}
                disabled={!memoriesEnabled}
                onClick={() => persist({ ...tools, memories: { enabled: false } })}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </div>
          )}
        </div>

        <div className="space-y-2 py-1.5">
          <div className="flex items-center gap-3">
            <SlackMark className="size-4 shrink-0 text-muted-foreground" />
            <p className="min-w-0 flex-1 text-body">
              {t(($) => $.settings.tools_slack)}
              {!slackConnected && (
                <span className="ml-2 text-caption text-amber-600 dark:text-amber-400">
                  {t(($) => $.settings.tools_requires_connection)}
                </span>
              )}
            </p>
            {!slackConnected ? (
              <Button
                size="sm"
                className="h-7 px-2.5"
                nativeButton={false}
                render={
                  <AppLink href={settingsPathForTriggerProvider(wsPaths.settings(), "slack")} />
                }
              >
                {t(($) => $.settings.tools_connect)}
                <ExternalLink className="size-3.5" />
              </Button>
            ) : canWrite ? (
              <Switch
                size="sm"
                checked={slackEnabled}
                onCheckedChange={(enabled) =>
                  persist({
                    ...tools,
                    slack_send: { enabled, channel: tools.slack_send?.channel },
                  })
                }
              />
            ) : null}
          </div>
          {canWrite && slackConnected && slackEnabled && (
            <label className="block space-y-1 pl-7">
              <span className="text-caption text-muted-foreground">
                {t(($) => $.settings.tools_slack_channel)}
              </span>
              <Input
                key={tools.slack_send?.channel ?? ""}
                className="h-8"
                defaultValue={tools.slack_send?.channel ?? ""}
                placeholder={t(($) => $.settings.tools_slack_channel_placeholder)}
                onBlur={(event) => {
                  persist({
                    ...tools,
                    slack_send: {
                      enabled: true,
                      channel: event.target.value.trim() || undefined,
                    },
                  });
                }}
              />
            </label>
          )}
        </div>

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
              <p className="text-caption text-muted-foreground">
                {t(($) => $.settings.tools_mcp_hint)}
              </p>
              {servers.length === 0 ? (
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
            </PopoverContent>
          </Popover>
        )}
      </div>
    </section>
  );
}
