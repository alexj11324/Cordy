"use client";

import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { parseAutomationTools, type AutomationToolsConfig } from "@patchbay/core/automations";
import { useUpdateAutomation } from "@patchbay/core/automations/mutations";
import { workspaceMcpServersOptions } from "@patchbay/core/workspace/queries";
import { useWorkspaceId } from "@patchbay/core/hooks";
import type { Automation } from "@patchbay/core/types";
import { Checkbox } from "@patchbay/ui/components/ui/checkbox";
import { Input } from "@patchbay/ui/components/ui/input";
import { Switch } from "@patchbay/ui/components/ui/switch";
import { toast } from "sonner";
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
  const updateAutomation = useUpdateAutomation();
  const tools = useMemo(() => parseAutomationTools(automation.tools), [automation.tools]);
  const mcpQuery = useQuery(workspaceMcpServersOptions(wsId));
  const servers = mcpQuery.data ?? [];

  const persist = (next: AutomationToolsConfig) => {
    updateAutomation.mutate(
      { id: automation.id, tools: next },
      {
        onSuccess: () => toast.success(t(($) => $.settings.toast_tools_updated)),
        onError: (err) => {
          toast.error(err instanceof Error ? err.message : t(($) => $.dialog.toast_update_failed));
        },
      },
    );
  };

  return (
    <section className="space-y-3">
      <h2 className="text-body font-medium text-muted-foreground uppercase tracking-wider">
        {t(($) => $.settings.section_tools)}
      </h2>
      <div className="rounded-lg border divide-y">
        <ToolRow
          title={t(($) => $.settings.tools_memories)}
          hint={t(($) => $.settings.tools_memories_hint)}
          checked={tools.memories?.enabled === true}
          disabled={!canWrite}
          onCheckedChange={(enabled) => persist({ ...tools, memories: { enabled } })}
        />
        <div className="px-4 py-3 space-y-2">
          <ToolRow
            title={t(($) => $.settings.tools_slack)}
            hint={t(($) => $.settings.tools_slack_hint)}
            checked={tools.slack_send?.enabled === true}
            disabled={!canWrite}
            onCheckedChange={(enabled) =>
              persist({
                ...tools,
                slack_send: { enabled, channel: tools.slack_send?.channel },
              })
            }
            bare
          />
          {tools.slack_send?.enabled === true && (
            <label className="block space-y-1">
              <span className="text-caption text-muted-foreground">
                {t(($) => $.settings.tools_slack_channel)}
              </span>
              <Input
                key={tools.slack_send.channel ?? ""}
                className="h-8"
                defaultValue={tools.slack_send.channel ?? ""}
                placeholder={t(($) => $.settings.tools_slack_channel_placeholder)}
                disabled={!canWrite}
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
        <div className="px-4 py-3 space-y-2">
          <div>
            <p className="text-body font-medium">{t(($) => $.settings.tools_mcp)}</p>
            <p className="text-caption text-muted-foreground">{t(($) => $.settings.tools_mcp_hint)}</p>
          </div>
          {servers.length === 0 ? (
            <p className="text-caption text-muted-foreground">{t(($) => $.settings.tools_mcp_empty)}</p>
          ) : (
            <ul className="space-y-1.5">
              {servers.map((server) => {
                const selected = tools.mcp_server_ids?.includes(server.id) === true;
                return (
                  <li key={server.id} className="flex items-center gap-2">
                    <Checkbox
                      checked={selected}
                      disabled={!canWrite}
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
        </div>
      </div>
    </section>
  );
}

function ToolRow({
  title,
  hint,
  checked,
  disabled,
  onCheckedChange,
  bare,
}: {
  title: string;
  hint: string;
  checked: boolean;
  disabled: boolean;
  onCheckedChange: (checked: boolean) => void;
  bare?: boolean;
}) {
  const body = (
    <div className="flex items-start justify-between gap-3">
      <div className="min-w-0">
        <p className="text-body font-medium">{title}</p>
        <p className="text-caption text-muted-foreground">{hint}</p>
      </div>
      <Switch size="sm" checked={checked} disabled={disabled} onCheckedChange={onCheckedChange} />
    </div>
  );
  if (bare) return body;
  return <div className="px-4 py-3">{body}</div>;
}
