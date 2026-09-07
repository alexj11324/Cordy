"use client";

import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ExternalLink, Trash2 } from "lucide-react";
import type { AutomationToolsConfig } from "@orvilo/core/automations";
import type { SlackAutomationChannel, SlackInstallation } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { Checkbox } from "@orvilo/ui/components/ui/checkbox";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@orvilo/ui/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@orvilo/ui/components/ui/select";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { AppLink } from "../../navigation";
import { SlackMark } from "../../settings/components/slack-mark";
import { useT } from "../../i18n";

function resolveConfiguredTarget(
  config: AutomationToolsConfig["slack_send"],
  installations: SlackInstallation[],
  channels: SlackAutomationChannel[],
): { installationId: string; channelIds: string[] } | undefined {
  if (!config) return undefined;
  const installedIds = new Set(installations.map((installation) => installation.id));
  const configuredInstallationId = config.installation_id && installedIds.has(config.installation_id)
    ? config.installation_id
    : undefined;

  if (configuredInstallationId) {
    const validIds = new Set(
      channels
        .filter((channel) => channel.installation_id === configuredInstallationId)
        .map((channel) => channel.id),
    );
    const channelIds = (config.channel_ids ?? []).filter((id) => validIds.has(id));
    if (channelIds.length > 0) return { installationId: configuredInstallationId, channelIds };
  }

  const legacy = config.channel?.trim();
  if (!legacy) {
    return configuredInstallationId
      ? { installationId: configuredInstallationId, channelIds: [] }
      : undefined;
  }
  const legacyName = legacy.startsWith("#") ? legacy.slice(1) : legacy;
  const matches = channels.filter((channel) =>
    installedIds.has(channel.installation_id) &&
    (!config.installation_id || channel.installation_id === config.installation_id) &&
    (channel.id === legacy || channel.name === legacyName),
  );
  return matches.length === 1
    ? { installationId: matches[0]!.installation_id, channelIds: [matches[0]!.id] }
    : configuredInstallationId
      ? { installationId: configuredInstallationId, channelIds: [] }
      : undefined;
}

export function AutomationSlackToolRow({
  tools,
  installations,
  channels,
  canWrite,
  busy,
  catalogPending,
  catalogError,
  connectHref,
  onRetry,
  onPersist,
  onRemove,
}: {
  tools: AutomationToolsConfig;
  installations: SlackInstallation[];
  channels: SlackAutomationChannel[];
  canWrite: boolean;
  busy: boolean;
  catalogPending: boolean;
  catalogError: boolean;
  connectHref: string;
  onRetry: () => void;
  onPersist: (next: AutomationToolsConfig) => void;
  onRemove: () => void;
}) {
  const { t } = useT("automations");
  const installed = useMemo(
    () => installations.filter((row) =>
      (row.installation_status ?? row.status) === "installed"),
    [installations],
  );
  const configuredTarget = useMemo(
    () => resolveConfiguredTarget(tools.slack_send, installed, channels),
    [channels, installed, tools.slack_send],
  );
  const onlyInstallationId = installed.length === 1 ? installed[0]!.id : "";
  const [installationId, setInstallationId] = useState(
    configuredTarget?.installationId ?? onlyInstallationId,
  );
  const [channelIds, setChannelIds] = useState(configuredTarget?.channelIds ?? []);

  const configuredKey = `${configuredTarget?.installationId ?? ""}:${configuredTarget?.channelIds.join(",") ?? ""}:${onlyInstallationId}`;
  useEffect(() => {
    setInstallationId(configuredTarget?.installationId ?? onlyInstallationId);
    setChannelIds(configuredTarget?.channelIds ?? []);
  }, [configuredKey, configuredTarget?.channelIds, configuredTarget?.installationId, onlyInstallationId]);

  const availableChannels = channels.filter((channel) => channel.installation_id === installationId);
  const availableIds = new Set(availableChannels.map((channel) => channel.id));
  const selectedIds = [...new Set(channelIds.filter((id) => availableIds.has(id)))];
  const selectedNames = selectedIds.map(
    (id) => availableChannels.find((channel) => channel.id === id)?.name,
  ).filter((name): name is string => Boolean(name));
  const channelLabel = selectedNames.length === 0
    ? t(($) => $.settings.tools_slack_select_channels)
    : selectedNames.length === 1
      ? `#${selectedNames[0]}`
      : t(($) => $.settings.tools_slack_channels_selected, { count: selectedNames.length });

  const persistTarget = (nextInstallationId: string, nextChannelIds: string[]) => {
    onPersist({
      ...tools,
      slack_send: {
        installation_id: nextInstallationId || undefined,
        channel_ids: nextChannelIds,
      },
    });
  };

  const changeInstallation = (nextInstallationId: string) => {
    setInstallationId(nextInstallationId);
    setChannelIds([]);
  };

  const changeChannels = (nextChannelIds: string[]) => {
    setChannelIds(nextChannelIds);
    if (nextChannelIds.length > 0) persistTarget(installationId, nextChannelIds);
  };

  return (
    <div className="space-y-2 py-1.5">
      <div className="flex items-center gap-3">
        <SlackMark className="size-4 shrink-0 text-muted-foreground" />
        <p className="min-w-0 flex-1 text-body">
          {t(($) => $.settings.tools_slack)}
          {!catalogPending && !catalogError && installed.length === 0 && (
            <span className="ml-2 text-caption text-amber-600 dark:text-amber-400">
              {t(($) => $.settings.tools_requires_connection)}
            </span>
          )}
        </p>
        {canWrite && (
          <div className="flex shrink-0 items-center gap-1">
            {catalogPending && <Skeleton className="h-7 w-16" />}
            {catalogError && (
              <Button size="sm" variant="ghost" onClick={onRetry}>{t(($) => $.page.retry)}</Button>
            )}
            {!catalogPending && !catalogError && installed.length === 0 && (
              <Button
                size="sm"
                className="h-7 px-2.5"
                nativeButton={false}
                render={<AppLink href={connectHref} />}
              >
                {t(($) => $.settings.tools_connect)}
                <ExternalLink className="size-3.5" />
              </Button>
            )}
            <Button
              size="icon-sm"
              variant="ghost"
              className="text-muted-foreground hover:text-destructive"
              aria-label={t(($) => $.settings.tools_slack_remove)}
              disabled={busy}
              onClick={onRemove}
            >
              <Trash2 className="size-3.5" />
            </Button>
          </div>
        )}
      </div>

      {catalogError && (
        <p role="status" className="pl-7 text-caption text-destructive">
          {t(($) => $.settings.tools_slack_catalog_failed)}
        </p>
      )}
      {!catalogPending && !catalogError && installed.length > 0 && (
        <div className="flex flex-wrap items-center gap-2 pl-7">
          {installed.length > 1 && (
            <Select
              value={installationId || null}
              items={installed.map((row) => ({
                value: row.id,
                label: t(($) => $.settings.tools_slack_workspace_label, { team: row.team_id }),
              }))}
              onValueChange={(value) => { if (value) changeInstallation(value); }}
            >
              <SelectTrigger
                size="sm"
                aria-label={t(($) => $.settings.tools_slack_workspace)}
                disabled={!canWrite || busy}
              >
                <SelectValue placeholder={t(($) => $.settings.tools_slack_select_workspace)} />
              </SelectTrigger>
              <SelectContent align="start">
                {installed.map((row) => (
                  <SelectItem key={row.id} value={row.id}>
                    {t(($) => $.settings.tools_slack_workspace_label, { team: row.team_id })}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}

          <Popover>
            <PopoverTrigger render={
              <Button
                size="sm"
                variant="secondary"
                className="h-7 max-w-56 gap-1 px-2 font-normal"
                aria-label={t(($) => $.settings.tools_slack_channels)}
                disabled={!canWrite || busy || !installationId}
              >
                <span className="truncate">{channelLabel}</span>
                <ChevronDown className="size-3" />
              </Button>
            } />
            <PopoverContent align="start" className="max-h-72 w-72 overflow-auto">
              {availableChannels.length === 0 ? (
                <p className="text-caption text-muted-foreground">
                  {t(($) => $.settings.tools_slack_no_channels)}
                </p>
              ) : (
                <ul className="space-y-1.5">
                  {availableChannels.map((channel) => {
                    const selected = selectedIds.includes(channel.id);
                    return (
                      <li key={channel.id} className="flex items-center gap-2">
                        <Checkbox
                          checked={selected}
                          aria-label={`#${channel.name}`}
                          disabled={busy || (selected && selectedIds.length === 1)}
                          onCheckedChange={(checked) => {
                            if (busy) return;
                            const next = new Set(selectedIds);
                            if (checked === true) next.add(channel.id);
                            else next.delete(channel.id);
                            changeChannels([...next]);
                          }}
                        />
                        <span className="min-w-0 truncate text-body">#{channel.name}</span>
                      </li>
                    );
                  })}
                </ul>
              )}
            </PopoverContent>
          </Popover>
        </div>
      )}

    </div>
  );
}
