"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, Clock, ExternalLink, RotateCw, Trash2, Webhook } from "lucide-react";
import { toast } from "sonner";
import {
  automationTriggerPreset,
  buildAutomationWebhookUrl,
  isNativeAutomationProvider,
  parseAutomationTriggerConfig,
  settingsPathForTriggerProvider,
  type AutomationTriggerConfig,
} from "@patchbay/core/automations";
import { githubInstallationsOptions } from "@patchbay/core/github/queries";
import { slackInstallationsOptions } from "@patchbay/core/slack/queries";
import { linearConnectionOptions } from "@patchbay/core/linear/queries";
import {
  useDeleteAutomationTrigger,
  useRotateAutomationTriggerWebhookToken,
  useUpdateAutomationTrigger,
} from "@patchbay/core/automations/mutations";
import { api } from "@patchbay/core/api";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import type { AutomationTrigger } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { Checkbox } from "@patchbay/ui/components/ui/checkbox";
import { Input } from "@patchbay/ui/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@patchbay/ui/components/ui/select";
import { Switch } from "@patchbay/ui/components/ui/switch";
import { cn } from "@patchbay/ui/lib/utils";
import { AppLink } from "../../navigation";
import { GitHubMark } from "../../settings/components/github-mark";
import { useDescribeSchedule } from "./schedule-editor/describe";
import { parseCron, toCron } from "./schedule-editor/cron-mapping";
import { pad2 } from "./schedule-editor/model";
import { WebhookUrlField } from "./webhook-url-field";
import { useT } from "../../i18n";
import { formatInTimeZone } from "../../common/format-in-time-zone";
import { SlackMark } from "../../settings/components/slack-mark";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@patchbay/ui/components/ui/alert-dialog";

function compactConfig(config: AutomationTriggerConfig): AutomationTriggerConfig {
  const next: AutomationTriggerConfig = {};
  const channel = config.channel?.trim();
  const keyword = config.keyword?.trim();
  const regex = config.regex?.trim();
  const emoji = config.emoji?.trim();
  const branch = config.branch?.trim();
  const label = config.label?.trim();
  if (channel) next.channel = channel;
  if (keyword) next.keyword = keyword;
  if (regex) next.regex = regex;
  if (emoji) next.emoji = emoji;
  if (branch) next.branch = branch;
  if (label) next.label = label;
  if (config.on_failure === true) next.on_failure = true;
  return next;
}

function sameConfig(a: AutomationTriggerConfig, b: AutomationTriggerConfig): boolean {
  return JSON.stringify(compactConfig(a)) === JSON.stringify(compactConfig(b));
}

function filterSummary(config: AutomationTriggerConfig): string {
  const parts: string[] = [];
  if (config.channel) parts.push(config.channel);
  if (config.keyword) parts.push(config.keyword);
  if (config.branch) parts.push(config.branch);
  if (config.label) parts.push(config.label);
  if (config.emoji) parts.push(config.emoji);
  if (config.on_failure === true) parts.push("failure");
  return parts.join(" · ");
}

function hasConfigurableFilters(trigger: AutomationTrigger, provider: string | null): boolean {
  if (trigger.preset === "slack.message" || trigger.preset === "slack.reaction") return true;
  if (provider === "github") return true;
  return false;
}

function shortTimeZoneName(timeZone: string, locale: string): string {
  try {
    const parts = new Intl.DateTimeFormat(locale, {
      timeZone,
      timeZoneName: "short",
    }).formatToParts(new Date());
    return parts.find((part) => part.type === "timeZoneName")?.value ?? "";
  } catch {
    return "";
  }
}

function dailyAtTime(config: ReturnType<typeof parseCron> | null): string | null {
  if (!config || config.raw !== null) return null;
  if (config.time.kind !== "at" || config.days.kind !== "every") return null;
  return config.time.time;
}

function scheduleTimeItems(current: string): { value: string; label: string }[] {
  const items: { value: string; label: string }[] = [];
  for (let hour = 0; hour < 24; hour += 1) {
    for (const minute of [0, 30] as const) {
      const value = `${pad2(hour)}:${pad2(minute)}`;
      items.push({ value, label: value });
    }
  }
  if (!items.some((item) => item.value === current)) {
    items.push({ value: current, label: current });
    items.sort((a, b) => a.value.localeCompare(b.value));
  }
  return items;
}

export function TriggerCard({
  trigger,
  automationId,
  canWrite,
}: {
  trigger: AutomationTrigger;
  automationId: string;
  canWrite: boolean;
}) {
  const { t, i18n } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const describeSchedule = useDescribeSchedule();
  const updateTrigger = useUpdateAutomationTrigger();
  const deleteTrigger = useDeleteAutomationTrigger();
  const rotateToken = useRotateAutomationTriggerWebhookToken();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [rotateOpen, setRotateOpen] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const preset = automationTriggerPreset(trigger.preset);
  const title = preset
    ? (t(($) => $.presets[preset.labelKey as keyof typeof $.presets]) || preset.id)
    : t(($) => $.trigger_kind[trigger.kind]);
  const parsed = useMemo(
    () => parseAutomationTriggerConfig(trigger.config),
    [trigger.config],
  );
  const [config, setConfig] = useState<AutomationTriggerConfig>(parsed);
  const pendingConfigRef = useRef<AutomationTriggerConfig | null>(null);
  const saveTimerRef = useRef<number | null>(null);

  const flushPendingConfig = useCallback(() => {
    const pending = pendingConfigRef.current;
    if (!canWrite || !pending) return;
    pendingConfigRef.current = null;
    if (sameConfig(pending, parsed)) return;
    updateTrigger.mutate(
      { automationId, triggerId: trigger.id, config: { ...pending } },
      {
        onError: (err) => {
          toast.error(err instanceof Error ? err.message : t(($) => $.trigger_row.toast_delete_failed));
        },
      },
    );
  }, [automationId, canWrite, parsed, t, trigger.id, updateTrigger]);

  const latestFlushRef = useRef(flushPendingConfig);
  useEffect(() => {
    latestFlushRef.current = flushPendingConfig;
  }, [flushPendingConfig]);

  // A route change unmounts the card before the debounce timer fires. Flush
  // the last compacted value so the autosave promise is not lost on navigation.
  useEffect(() => () => latestFlushRef.current(), []);

  useEffect(() => {
    setConfig(parsed);
  }, [parsed]);

  useEffect(() => {
    if (saveTimerRef.current !== null) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = null;
    }
    if (!canWrite || sameConfig(config, parsed)) {
      pendingConfigRef.current = null;
      return;
    }
    pendingConfigRef.current = compactConfig(config);
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = null;
      flushPendingConfig();
    }, 500);
    return () => {
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
    };
  }, [canWrite, config, flushPendingConfig, parsed]);

  const github = useQuery(githubInstallationsOptions(wsId));
  const slack = useQuery(slackInstallationsOptions(wsId));
  const linear = useQuery(linearConnectionOptions(wsId));
  const provider = trigger.provider ?? preset?.provider ?? null;
  const native = isNativeAutomationProvider(provider);
  const githubConnected = (github.data?.installations.length ?? 0) > 0;
  const slackConnected = (slack.data?.installations ?? []).some(
    (row) => row.status === "installed" || row.installation_status === "installed",
  );
  const linearConnected = linear.data?.connected === true;
  const connected =
    provider === "github" ? githubConnected :
    provider === "slack" ? slackConnected :
    provider === "linear" ? linearConnected :
    true;

  const webhookUrl = trigger.kind === "webhook" && trigger.webhook_token
    ? buildAutomationWebhookUrl({
        trigger,
        apiBaseUrl: api.getBaseUrl(),
        currentOrigin: typeof window !== "undefined" ? window.location.origin : undefined,
      })
    : null;
  const scheduleConfig = trigger.cron_expression
    ? parseCron(trigger.cron_expression, trigger.timezone ?? "UTC")
    : null;
  const scheduleDescription = scheduleConfig ? describeSchedule(scheduleConfig) : null;
  const showFilters = hasConfigurableFilters(trigger, provider);
  const summary = filterSummary(compactConfig(config));
  const dailyTime = dailyAtTime(scheduleConfig);
  const timeItems = dailyTime ? scheduleTimeItems(dailyTime) : [];
  const primaryTitle = dailyTime
    ? t(($) => $.settings.schedule_every_day_at)
    : (scheduleDescription ?? title);
  const tzShort =
    trigger.kind === "schedule"
      ? shortTimeZoneName(trigger.timezone ?? "UTC", i18n.language)
      : "";
  const nextRun =
    trigger.kind === "schedule" && trigger.next_run_at
      ? t(($) => $.settings.next_run, {
          date: formatInTimeZone(trigger.next_run_at, trigger.timezone ?? undefined, i18n.language),
        })
      : (!dailyTime && !scheduleDescription && summary ? summary : null);
  const needsConnection = native && !connected;

  const handleDelete = async () => {
    try {
      await deleteTrigger.mutateAsync({ automationId, triggerId: trigger.id });
      toast.success(t(($) => $.trigger_row.toast_deleted));
      setConfirmOpen(false);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t(($) => $.trigger_row.toast_delete_failed));
    }
  };

  const handleRotate = async () => {
    try {
      await rotateToken.mutateAsync({ automationId, triggerId: trigger.id });
      toast.success(t(($) => $.trigger_row.toast_rotated));
      setRotateOpen(false);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t(($) => $.trigger_row.toast_rotate_failed));
    }
  };

  return (
    <div className="group/trigger px-3.5 py-3">
      <div className="flex items-center gap-2">
        <TriggerGlyph provider={provider} kind={trigger.kind} />
        <div className="flex min-w-0 flex-1 items-center gap-1.5">
          <p className="shrink-0 text-body">{primaryTitle}</p>
          {dailyTime && scheduleConfig && (
            <div className="inline-flex h-6 shrink-0 items-center rounded-md bg-muted px-1.5">
              <Select
                items={timeItems}
                value={dailyTime}
                disabled={!canWrite}
                onValueChange={(time) => {
                  if (!canWrite || !time || time === dailyTime) return;
                  updateTrigger.mutate({
                    automationId,
                    triggerId: trigger.id,
                    cron_expression: toCron({
                      ...scheduleConfig,
                      time: { kind: "at", time },
                    }),
                  });
                }}
              >
                <SelectTrigger
                  size="sm"
                  aria-label={t(($) => $.settings.schedule_time_aria)}
                  className="h-6 min-w-0 gap-0.5 border-0 bg-transparent p-0 text-body shadow-none ring-0 dark:bg-transparent [&_svg]:size-3"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent align="start" className="max-h-64 min-w-24">
                  {timeItems.map((item) => (
                    <SelectItem key={item.value} value={item.value}>
                      {item.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          {tzShort ? (
            <span className="shrink-0 text-body text-muted-foreground">{tzShort}</span>
          ) : null}
          {!trigger.enabled && !needsConnection && (
            <span className="shrink-0 text-caption text-muted-foreground">
              {t(($) => $.trigger_row.disabled_badge)}
            </span>
          )}
          {needsConnection && (
            <span className="shrink-0 text-caption text-amber-600 dark:text-amber-400">
              {t(($) => $.settings.tools_requires_connection)}
            </span>
          )}
          {nextRun && (
            <span className="min-w-0 truncate text-caption text-muted-foreground">
              {nextRun}
            </span>
          )}
        </div>
        {canWrite && (
          <div className="ml-auto flex shrink-0 items-center gap-1">
            {needsConnection && provider && (
              <Button
                size="sm"
                className="h-7 px-2.5"
                nativeButton={false}
                render={
                  <AppLink href={settingsPathForTriggerProvider(wsPaths.settings(), provider)} />
                }
              >
                {t(($) => $.settings.tools_connect)}
                <ExternalLink className="size-3.5" />
              </Button>
            )}
            <div
              className={cn(
                "flex items-center gap-1",
                filtersOpen
                  ? "opacity-100"
                  : "opacity-0 group-hover/trigger:opacity-100 focus-within:opacity-100",
              )}
            >
              {showFilters && (
                <Button
                  size="icon-sm"
                  variant="ghost"
                  className="text-muted-foreground"
                  onClick={() => setFiltersOpen((open) => !open)}
                  aria-expanded={filtersOpen}
                  aria-label={t(($) => $.settings.configure_filters)}
                >
                  {filtersOpen ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                </Button>
              )}
              {!needsConnection && (
                <Switch
                  size="sm"
                  checked={trigger.enabled}
                  onCheckedChange={(checked) => {
                    updateTrigger.mutate({ automationId, triggerId: trigger.id, enabled: checked });
                  }}
                  aria-label={title}
                />
              )}
              <Button
                size="icon-sm"
                variant="ghost"
                className="text-muted-foreground hover:text-destructive"
                onClick={() => setConfirmOpen(true)}
                aria-label={t(($) => $.trigger_row.delete_dialog.title)}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </div>
          </div>
        )}
      </div>

      {webhookUrl && (
        <div className="mt-2">
          <WebhookUrlField
            url={webhookUrl}
            actions={
              canWrite ? (
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-7 w-7 shrink-0"
                  onClick={() => setRotateOpen(true)}
                  title={t(($) => $.trigger_row.rotate_url)}
                  disabled={rotateToken.isPending}
                >
                  <RotateCw className="h-3.5 w-3.5 text-muted-foreground" />
                </Button>
              ) : undefined
            }
          />
        </div>
      )}

      {canWrite && native && filtersOpen && (
        <div className="mt-3 grid gap-2 sm:grid-cols-2">
          {(trigger.preset === "slack.message" || trigger.preset === "slack.reaction") && (
            <>
              <ConfigField
                label={t(($) => $.settings.config_channel)}
                value={config.channel ?? ""}
                onChange={(channel) => setConfig((prev) => ({ ...prev, channel }))}
                onBlur={flushPendingConfig}
                // Slack channel IDs are technical values, not translatable copy.
                // eslint-disable-next-line no-restricted-syntax
                placeholder="C123…"
              />
              <ConfigField
                label={t(($) => $.settings.config_keyword)}
                value={config.keyword ?? ""}
                onChange={(keyword) => setConfig((prev) => ({ ...prev, keyword }))}
                onBlur={flushPendingConfig}
              />
            </>
          )}
          {trigger.preset === "slack.message" && (
            <ConfigField
              label={t(($) => $.settings.config_regex)}
              value={config.regex ?? ""}
              onChange={(regex) => setConfig((prev) => ({ ...prev, regex }))}
              onBlur={flushPendingConfig}
            />
          )}
          {trigger.preset === "slack.reaction" && (
            <ConfigField
              label={t(($) => $.settings.config_emoji)}
              value={config.emoji ?? ""}
              onChange={(emoji) => setConfig((prev) => ({ ...prev, emoji }))}
              onBlur={flushPendingConfig}
            />
          )}
          {provider === "github" && (
            <>
              <ConfigField
                label={t(($) => $.settings.config_branch)}
                value={config.branch ?? ""}
                onChange={(branch) => setConfig((prev) => ({ ...prev, branch }))}
                onBlur={flushPendingConfig}
              />
              <ConfigField
                label={t(($) => $.settings.config_label)}
                value={config.label ?? ""}
                onChange={(label) => setConfig((prev) => ({ ...prev, label }))}
                onBlur={flushPendingConfig}
              />
            </>
          )}
          {(trigger.preset === "github.ci_completed" ||
            trigger.preset === "github.workflow_run.completed") && (
            <label className="flex items-center gap-2 sm:col-span-2">
              <Checkbox
                checked={config.on_failure === true}
                onCheckedChange={(checked) =>
                  setConfig((prev) => ({ ...prev, on_failure: checked === true }))
                }
              />
              <span className="text-body">{t(($) => $.settings.config_on_failure)}</span>
            </label>
          )}
        </div>
      )}

      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t(($) => $.trigger_row.delete_dialog.title)}</AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.trigger_row.delete_dialog.description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t(($) => $.trigger_row.delete_dialog.cancel)}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete} className="bg-destructive text-white hover:bg-destructive/90">
              {t(($) => $.trigger_row.delete_dialog.confirm)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={rotateOpen} onOpenChange={setRotateOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t(($) => $.trigger_row.rotate_confirm_title)}</AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.trigger_row.rotate_confirm_description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t(($) => $.trigger_row.rotate_confirm_cancel)}</AlertDialogCancel>
            <AlertDialogAction onClick={handleRotate}>
              {t(($) => $.trigger_row.rotate_confirm_action)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function TriggerGlyph({
  provider,
  kind,
}: {
  provider: string | null;
  kind: AutomationTrigger["kind"];
}) {
  const className = "size-4 shrink-0 text-muted-foreground";
  if (kind === "schedule") return <Clock className={className} />;
  if (provider === "slack") return <SlackMark className={className} />;
  if (provider === "github") return <GitHubMark className={className} />;
  return <Webhook className={className} />;
}

function ConfigField({
  label,
  value,
  onChange,
  onBlur,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  onBlur?: () => void;
  placeholder?: string;
}) {
  return (
    <label className="space-y-1">
      <span className="text-caption text-muted-foreground">{label}</span>
      <Input value={value} placeholder={placeholder} onChange={(event) => onChange(event.target.value)} onBlur={onBlur} className="h-8" />
    </label>
  );
}
