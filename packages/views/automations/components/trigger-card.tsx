"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, Clock, ExternalLink, Pencil, RotateCw, Trash2, Webhook } from "lucide-react";
import { toast } from "sonner";
import {
  automationTriggerPreset,
  buildAutomationWebhookUrl,
  isNativeAutomationProvider,
  parseAutomationTriggerConfig,
  settingsPathForTriggerProvider,
  type AutomationTriggerConfig,
} from "@orvilo/core/automations";
import { githubAutomationRepositoriesOptions, githubInstallationsOptions } from "@orvilo/core/github/queries";
import { slackAutomationCatalogOptions, slackInstallationsOptions } from "@orvilo/core/slack/queries";
import { linearCatalogOptions, linearConnectionOptions } from "@orvilo/core/linear/queries";
import {
  useDeleteAutomationTrigger,
  useRotateAutomationTriggerWebhookToken,
  useUpdateAutomationTrigger,
} from "@orvilo/core/automations/mutations";
import { api } from "@orvilo/core/api";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import type { AutomationTrigger } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { Checkbox } from "@orvilo/ui/components/ui/checkbox";
import { Input } from "@orvilo/ui/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@orvilo/ui/components/ui/select";
import { Popover, PopoverContent, PopoverTrigger } from "@orvilo/ui/components/ui/popover";
import { AppLink } from "../../navigation";
import { GitHubMark } from "../../settings/components/github-mark";
import { useDescribeSchedule } from "./schedule-editor/describe";
import { parseCron, toCron } from "./schedule-editor/cron-mapping";
import { pad2 } from "./schedule-editor/model";
import { WebhookUrlField } from "./webhook-url-field";
import { useT } from "../../i18n";
import { formatInTimeZone } from "../../common/format-in-time-zone";
import { SlackMark } from "../../settings/components/slack-mark";
import { LinearMark } from "../../settings/components/linear-mark";
import { TriggerScheduleDialog } from "./trigger-schedule-dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@orvilo/ui/components/ui/alert-dialog";

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
  if (config.repository?.trim()) next.repository = config.repository.trim();
  if (config.review_state) next.review_state = config.review_state;
  if (config.thread_state) next.thread_state = config.thread_state;
  if (config.conclusion) next.conclusion = config.conclusion;
  if (config.repositories?.length) next.repositories = [...new Set(config.repositories.map((value) => value.trim()).filter(Boolean))];
  if (config.author_scope && config.author_scope !== "anyone") next.author_scope = config.author_scope;
  if (config.author_logins?.length) next.author_logins = [...new Set(config.author_logins.map((value) => value.trim()).filter(Boolean))];
  if (config.installation_id) next.installation_id = config.installation_id;
  if (config.sender_scope && config.sender_scope !== "anyone") next.sender_scope = config.sender_scope;
  if (typeof config.ignore_thread_replies === "boolean") next.ignore_thread_replies = config.ignore_thread_replies;
  if (config.completion_reaction) next.completion_reaction = config.completion_reaction.trim();
  if (config.team_id) next.team_id = config.team_id;
  if (config.project_id) next.project_id = config.project_id;
  if (config.status_id) next.status_id = config.status_id;
  return next;
}

function sameConfig(a: AutomationTriggerConfig, b: AutomationTriggerConfig): boolean {
  return JSON.stringify(compactConfig(a)) === JSON.stringify(compactConfig(b));
}

function filterSummary(config: AutomationTriggerConfig, valueLabel: (value: string) => string): string {
  const parts: string[] = [];
  if (config.channel) parts.push(config.channel);
  if (config.keyword) parts.push(config.keyword);
  if (config.regex) parts.push(config.regex);
  if (config.branch) parts.push(config.branch);
  if (config.label) parts.push(config.label);
  if (config.emoji) parts.push(config.emoji);
  if (config.repository) parts.push(config.repository);
  if (config.repositories?.length) parts.push(config.repositories.join(", "));
  if (config.review_state) parts.push(valueLabel(config.review_state));
  if (config.thread_state) parts.push(valueLabel(config.thread_state));
  if (config.conclusion) parts.push(valueLabel(config.conclusion));
  if (config.on_failure === true) parts.push(valueLabel("unsuccessful"));
  return parts.join(" · ");
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
  const [scheduleOpen, setScheduleOpen] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
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
  const savingConfigRef = useRef(false);
  const savedConfigRef = useRef(parsed);
  const failedConfigRef = useRef<AutomationTriggerConfig | null>(null);
  const latestFlushRef = useRef<() => void>(() => {});

  const flushPendingConfig = useCallback((retry = false) => {
    const pending = pendingConfigRef.current;
    if (!canWrite || !pending || savingConfigRef.current) return;
    if (!retry && failedConfigRef.current && sameConfig(pending, failedConfigRef.current)) return;
    pendingConfigRef.current = null;
    if (sameConfig(pending, savedConfigRef.current)) return;
    savingConfigRef.current = true;
    failedConfigRef.current = null;
    // Each PATCH replaces the whole config. Keep one request in flight so a
    // slower earlier save can never overwrite the user's newer filter edits.
    void updateTrigger.mutateAsync({ automationId, triggerId: trigger.id, config: { ...pending } })
      .then(() => {
        savedConfigRef.current = pending;
        setSaveError(null);
      })
      .catch((err: unknown) => {
        const message = err instanceof Error ? err.message : t(($) => $.dialog.toast_update_failed);
        setSaveError(message);
        toast.error(message);
        failedConfigRef.current = pending;
        // Preserve the draft for an explicit retry. A newer edit already in
        // the queue supersedes the failed value, including an invalid regex.
        pendingConfigRef.current ??= pending;
      })
      .finally(() => {
        savingConfigRef.current = false;
        const queued = pendingConfigRef.current;
        if (queued && !sameConfig(queued, pending)) latestFlushRef.current();
        else if (queued && sameConfig(queued, savedConfigRef.current)) pendingConfigRef.current = null;
      });
  }, [automationId, canWrite, t, trigger.id, updateTrigger]);

  useEffect(() => {
    latestFlushRef.current = flushPendingConfig;
  }, [flushPendingConfig]);

  // A route change unmounts the card before the debounce timer fires. Flush
  // the last compacted value so the autosave promise is not lost on navigation.
  useEffect(() => () => latestFlushRef.current(), []);

  useEffect(() => {
    if (savingConfigRef.current || pendingConfigRef.current) return;
    savedConfigRef.current = parsed;
    setConfig(parsed);
  }, [parsed]);

  useEffect(() => {
    if (saveTimerRef.current !== null) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = null;
    }
    if (!canWrite || (!savingConfigRef.current && sameConfig(config, savedConfigRef.current))) {
      pendingConfigRef.current = null;
      failedConfigRef.current = null;
      setSaveError(null);
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
  const connectionQuery = provider === "github" ? github : provider === "slack" ? slack : linear;
  const checkingConnection = native && connectionQuery.isPending;
  const connectionFailed = native && connectionQuery.isError;
  // A stale data payload must not make a failed provider query look connected.
  // The controls stay hidden until the current connection query has produced a
  // usable result; the retry action remains visible in the failed state.
  const connectionResolved = !connectionQuery.isPending && !connectionQuery.isError;
  const githubConnected = (github.data?.installations.length ?? 0) > 0;
  const slackConnected = (slack.data?.installations ?? []).some(
    (row) => row.status === "installed" || row.installation_status === "installed",
  );
  const linearConnected = linear.data?.connected === true;
  const connected =
    !native || !connectionResolved ? false :
    provider === "github" ? githubConnected :
    provider === "slack" ? slackConnected :
    provider === "linear" ? linearConnected :
    true;
  const githubRepositories = useQuery(githubAutomationRepositoriesOptions(wsId, automationId, provider === "github" && githubConnected));
  const slackCatalog = useQuery(slackAutomationCatalogOptions(wsId, provider === "slack" && slackConnected));
  const linearCatalog = useQuery(linearCatalogOptions(wsId, provider === "linear" && linearConnected));

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
  // Only offer fields present in this provider event's payload. For example,
  // Slack reactions have no message body and GitHub issue comments have no ref.
  const isPushToBranch = trigger.preset === "github.push_to_branch";
  const hasLabel = trigger.preset === "github.pull_request.label_changed"
    || trigger.preset === "github.issue.label_changed";
  const hasAuthor = trigger.preset === "github.pull_request.opened" || isPushToBranch;
  const showFilters = hasLabel || [
    "github.pull_request.review_submitted",
    "github.pull_request.review_thread",
    "github.ci_completed",
    "github.workflow_run.completed",
  ].includes(trigger.preset ?? "");
  const valueLabel = (value: string) => t(($) => $.settings.filter_values[value as keyof typeof $.settings.filter_values]) || value;
  const selectedRepositories = config.repositories ?? (config.repository ? [config.repository] : []);
  const ownGitHubLogins = githubRepositories.data?.me_logins ?? [];
  const githubRepositoryOptions = (githubRepositories.data?.repositories ?? []).map((repository) => ({
    value: repository.full_name,
    label: repository.full_name,
  }));
  const slackChannelOptions = (slackCatalog.data?.channels ?? []).map((channel) => ({
    value: `${channel.installation_id}|${channel.id}`,
    label: `#${channel.name} · ${channel.team_id}`,
  }));
  const slackInstallationOptions = (slack.data?.installations ?? [])
    .filter((installation) => installation.status === "installed" || installation.installation_status === "installed")
    .map((installation) => ({ value: installation.id, label: installation.team_id || installation.id }));
  const selectedSlackChannel = config.installation_id && config.channel
    ? `${config.installation_id}|${config.channel}`
    : "";
  const linearTeams = linearCatalog.data?.teams ?? [];
  const linearProjects = (linearCatalog.data?.projects ?? []).filter(
    (project) => !config.team_id || !project.team_id || project.team_id === config.team_id,
  );
  const linearStates = (linearCatalog.data?.states ?? []).filter(
    (state) => !config.team_id || !state.team_id || state.team_id === config.team_id,
  );
  const providerCatalog = provider === "github" ? githubRepositories : provider === "slack" ? slackCatalog : linearCatalog;
  const summary = filterSummary({
    branch: config.branch,
    label: config.label,
    review_state: config.review_state,
    thread_state: config.thread_state,
    conclusion: config.conclusion,
    on_failure: config.on_failure,
  }, valueLabel);
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
  const needsConnection = native && !connected && !checkingConnection && !connectionFailed;
  const updateError = (err: unknown) => toast.error(
    err instanceof Error ? err.message : t(($) => $.dialog.toast_update_failed),
  );

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
      <div className="flex flex-wrap items-center gap-2">
        <TriggerGlyph provider={provider} kind={trigger.kind} />
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
          <p className="break-words text-body">{primaryTitle}</p>
          {trigger.kind === "schedule" && !trigger.enabled && (
            <span className="text-caption text-amber-700 dark:text-amber-300">
              {t(($) => $.settings.trigger_needs_reconfiguration)}
            </span>
          )}
          {provider === "github" && connected && (
            <>
              <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_in)}</span>
              {isPushToBranch ? (
                <InlineConfigSelect
                  ariaLabel={t(($) => $.settings.config_repository)}
                  value={selectedRepositories[0] ?? "__select__"}
                  items={[
                    { value: "__select__", label: t(($) => $.settings.select_repository) },
                    ...githubRepositoryOptions,
                  ]}
                  disabled={!canWrite || !connected || githubRepositories.isPending || githubRepositories.isError}
                  onChange={(repository) => {
                    if (repository !== "__select__") setConfig((prev) => ({ ...prev, repository: undefined, repositories: [repository] }));
                  }}
                />
              ) : (
                <InlineMultiSelect
                  label={selectedRepositories.length > 0
                    ? t(($) => $.settings.repositories_selected, { count: selectedRepositories.length })
                    : t(($) => $.settings.select_repositories)}
                  ariaLabel={t(($) => $.settings.config_repository)}
                  options={githubRepositoryOptions}
                  values={selectedRepositories}
                  disabled={!canWrite || !connected || githubRepositories.isPending || githubRepositories.isError}
                  emptyLabel={githubRepositories.isError
                    ? t(($) => $.settings.catalog_unavailable)
                    : t(($) => $.settings.no_options)}
                  onChange={(repositories) => setConfig((prev) => ({ ...prev, repository: undefined, repositories }))}
                />
              )}
              {isPushToBranch && (
                <>
                  <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_on)}</span>
                  <BranchCondition value={config.branch ?? ""}
                    disabled={!canWrite || selectedRepositories.length === 0}
                    onChange={(branch) => setConfig((prev) => ({ ...prev, branch: branch || undefined }))} />
                </>
              )}
              {hasAuthor && (
                <>
                  <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_by)}</span>
                  <InlineConfigSelect
                    ariaLabel={t(($) => $.settings.config_author)}
                    value={config.author_scope ?? "anyone"}
                    items={[
                      { value: "anyone", label: t(($) => $.settings.anyone) },
                      ...(ownGitHubLogins.length > 0 ? [{ value: "me", label: t(($) => $.settings.me) }] : []),
                      { value: "specific", label: t(($) => $.settings.specific_people) },
                    ]}
                    disabled={!canWrite}
                    onChange={(author_scope) => {
                      if (author_scope === "specific") setFiltersOpen(true);
                      setConfig((prev) => ({
                        ...prev,
                        author_scope: author_scope as AutomationTriggerConfig["author_scope"],
                        author_logins: author_scope === "me" ? ownGitHubLogins : author_scope === "anyone" ? undefined : prev.author_logins,
                      }));
                    }}
                  />
                </>
              )}
            </>
          )}
          {trigger.preset === "slack.message" && connected && (
            <SlackMessageConditions
              config={config}
              canWrite={canWrite}
              connected={connected}
              channelOptions={slackChannelOptions}
              channelValue={selectedSlackChannel}
              catalogPending={slackCatalog.isPending}
              catalogError={slackCatalog.isError}
              onChange={setConfig}
            />
          )}
          {trigger.preset === "slack.reaction" && connected && (
            <>
              <EmojiCondition
                value={config.emoji ?? ""}
                disabled={!canWrite}
                onChange={(emoji) => setConfig((prev) => ({ ...prev, emoji: emoji || undefined }))}
              />
              <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_in)}</span>
              <SlackChannelSelect
                value={selectedSlackChannel}
                options={slackChannelOptions}
                disabled={!canWrite || !connected || slackCatalog.isPending || slackCatalog.isError}
                onChange={setConfig}
              />
              <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_from)}</span>
              <InlineConfigSelect
                ariaLabel={t(($) => $.settings.config_sender)}
                value={config.sender_scope ?? "anyone"}
                items={[
                  { value: "anyone", label: t(($) => $.settings.anyone) },
                  { value: "authenticated", label: t(($) => $.settings.authenticated_users) },
                ]}
                disabled={!canWrite}
                onChange={(sender_scope) => setConfig((prev) => ({
                  ...prev,
                  sender_scope: sender_scope as AutomationTriggerConfig["sender_scope"],
                }))}
              />
            </>
          )}
          {trigger.preset === "slack.channel_created" && connected && (
            <>
              <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_in)}</span>
              <InlineConfigSelect
                ariaLabel={t(($) => $.settings.slack_workspace)}
                value={config.installation_id ?? "any"}
                items={[
                  { value: "any", label: t(($) => $.settings.any_slack_workspace) },
                  ...slackInstallationOptions,
                ]}
                disabled={!canWrite || !connected}
                onChange={(installation) => setConfig((prev) => ({
                  ...prev, installation_id: installation === "any" ? undefined : installation,
                }))}
              />
            </>
          )}
          {provider === "linear" && connected && (
            <LinearConditions
              preset={trigger.preset ?? ""}
              config={config}
              canWrite={canWrite && connected && !linearCatalog.isPending && !linearCatalog.isError}
              teams={linearTeams}
              projects={linearProjects}
              states={linearStates}
              onChange={setConfig}
            />
          )}
          {connected && providerCatalog.isError && (
            <Button size="sm" variant="ghost" className="h-7 px-2 text-destructive"
              onClick={() => void providerCatalog.refetch()}>
              {t(($) => $.settings.catalog_unavailable)} · {t(($) => $.page.retry)}
            </Button>
          )}
          {provider === "slack" && connected && !slackCatalog.isPending && !slackCatalog.isError
            && slackChannelOptions.length === 0 && trigger.preset !== "slack.channel_created" && (
            <Button size="sm" variant="ghost" className="h-7 px-2 text-muted-foreground"
              onClick={() => void slackCatalog.refetch()}>
              {t(($) => $.settings.no_joined_slack_channels)} · {t(($) => $.page.retry)}
            </Button>
          )}
          {dailyTime && scheduleConfig && (
            <div className="inline-flex h-6 shrink-0 items-center rounded-md bg-muted px-1.5">
              <Select
                items={timeItems}
                value={dailyTime}
                disabled={!canWrite || updateTrigger.isPending}
                onValueChange={(time) => {
                  if (!canWrite || !time || time === dailyTime) return;
                  updateTrigger.mutate({
                    automationId,
                    triggerId: trigger.id,
                    cron_expression: toCron({
                      ...scheduleConfig,
                      time: { kind: "at", time },
                    }),
                  }, { onError: updateError });
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
          {needsConnection && (
            <span className="shrink-0 text-caption text-amber-600 dark:text-amber-400">
              {t(($) => $.settings.tools_requires_connection)}
            </span>
          )}
          {checkingConnection && <span className="text-caption text-muted-foreground">{t(($) => $.settings.checking_connection)}</span>}
          {connectionFailed && <span role="status" className="text-caption text-destructive">{t(($) => $.settings.connection_failed)}</span>}
          {nextRun && (!native || connected) && (
            <span className="min-w-0 truncate text-caption text-muted-foreground">
              {nextRun}
            </span>
          )}
        </div>
        {canWrite && (
          <div className="ml-auto flex shrink-0 items-center gap-1">
            {connectionFailed && (
              <Button size="sm" variant="ghost" onClick={() => void connectionQuery.refetch()}>{t(($) => $.page.retry)}</Button>
            )}
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
            <div className="grid grid-cols-[1.75rem_1.75rem] items-center gap-1">
              {trigger.kind === "schedule" && (
                <Button size="icon-sm" variant="ghost" className="col-start-1 row-start-1 text-muted-foreground"
                  onClick={() => setScheduleOpen(true)}
                  aria-label={t(($) => $.settings.edit_schedule)} title={t(($) => $.settings.edit_schedule)}>
                  <Pencil className="size-3.5" />
                </Button>
              )}
              {showFilters && (!native || connected) && (
                <Button
                  size="icon-sm"
                  variant="ghost"
                  className="col-start-1 row-start-1 text-muted-foreground"
                  onClick={() => setFiltersOpen((open) => !open)}
                  aria-expanded={filtersOpen}
                  aria-label={t(($) => $.settings.configure_filters)}
                  title={t(($) => $.settings.configure_filters)}
                >
                  {filtersOpen ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                </Button>
              )}
              <Button
                size="icon-sm"
                variant="ghost"
                className="col-start-2 row-start-1 text-muted-foreground hover:text-destructive"
                onClick={() => setConfirmOpen(true)}
                aria-label={t(($) => $.trigger_row.delete_dialog.title)}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </div>
          </div>
        )}
      </div>

      {webhookUrl && (!native || connected) && (
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

      {canWrite && native && connected && filtersOpen && (
        <div className="mt-3 grid gap-2 sm:grid-cols-2">
          {provider === "github" && config.author_scope === "specific" && <ConfigField
            label={t(($) => $.settings.specific_people)}
            value={config.author_logins?.join(", ") ?? ""}
            placeholder={t(($) => $.settings.github_login_placeholder)}
            onChange={(value) => setConfig((prev) => ({
              ...prev,
              author_logins: value.split(",").map((item) => item.trim()).filter(Boolean),
            }))}
            onBlur={flushPendingConfig}
          />}
          {trigger.preset === "github.pull_request.review_submitted" && <ConfigSelect
            label={t(($) => $.settings.config_review_state)}
            value={config.review_state ?? "any"}
            values={["any", "approved", "changes_requested", "commented"]}
            valueLabel={valueLabel}
            onChange={(value) => setConfig((prev) => ({ ...prev, review_state: value === "any" ? undefined : value }))}
          />}
          {trigger.preset === "github.pull_request.review_thread" && <ConfigSelect
            label={t(($) => $.settings.config_thread_state)}
            value={config.thread_state ?? "any"}
            values={["any", "resolved", "unresolved"]}
            valueLabel={valueLabel}
            onChange={(value) => setConfig((prev) => ({ ...prev, thread_state: value === "any" ? undefined : value }))}
          />}
          {hasLabel && (
              <ConfigField
                label={t(($) => $.settings.config_label)}
                value={config.label ?? ""}
                onChange={(label) => setConfig((prev) => ({ ...prev, label }))}
                onBlur={flushPendingConfig}
              />
          )}
          {(trigger.preset === "github.ci_completed" ||
            trigger.preset === "github.workflow_run.completed") && (
            <ConfigSelect
              label={t(($) => $.settings.config_conclusion)}
              value={config.on_failure ? "unsuccessful" : config.conclusion ?? "any"}
              values={["any", "success", "failure", "cancelled", "unsuccessful"]}
              valueLabel={valueLabel}
              onChange={(value) => setConfig((prev) => ({ ...prev,
                conclusion: value === "any" || value === "unsuccessful" ? undefined : value,
                on_failure: value === "unsuccessful",
              }))}
            />
          )}
        </div>
      )}
      {saveError && (!native || connected) && <div className="mt-2 flex items-center gap-2">
        <p role="alert" className="text-caption text-destructive">{saveError}</p>
        <Button size="sm" variant="ghost" disabled={updateTrigger.isPending} onClick={() => flushPendingConfig(true)}>{t(($) => $.page.retry)}</Button>
      </div>}
      {scheduleOpen && (
        <TriggerScheduleDialog automationId={automationId} trigger={trigger} onOpenChange={setScheduleOpen} />
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
  if (provider === "linear") return <LinearMark className={className} />;
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
      <Input value={value} placeholder={placeholder} onChange={(event) => onChange(event.target.value)} onBlur={() => onBlur?.()} className="h-8" />
    </label>
  );
}

function ConfigSelect({ label, value, values, valueLabel, onChange }: {
  label: string;
  value: string;
  values: string[];
  valueLabel: (value: string) => string;
  onChange: (value: string) => void;
}) {
  return <div className="space-y-1">
    <span className="text-caption text-muted-foreground">{label}</span>
    <Select value={value} items={values.map((item) => ({ value: item, label: valueLabel(item) }))}
      onValueChange={(next) => { if (next) onChange(next); }}>
      <SelectTrigger size="sm" aria-label={label} className="h-8 w-full"><SelectValue /></SelectTrigger>
      <SelectContent>{values.map((item) => <SelectItem key={item} value={item}>{valueLabel(item)}</SelectItem>)}</SelectContent>
    </Select>
  </div>;
}

function InlineConfigSelect({ ariaLabel, value, items, disabled, onChange }: {
  ariaLabel: string;
  value: string;
  items: { value: string; label: string }[];
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <Select items={items} value={value} disabled={disabled}
      onValueChange={(next) => { if (next !== null) onChange(next); }}>
      <SelectTrigger size="sm" aria-label={ariaLabel}
        className="h-7 min-w-0 max-w-56 gap-1 border-0 bg-muted px-2 text-body shadow-none dark:bg-muted">
        <SelectValue />
      </SelectTrigger>
      <SelectContent align="start">
        {items.map((item) => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}
      </SelectContent>
    </Select>
  );
}

function InlineMultiSelect({ label, ariaLabel, options, values, disabled, emptyLabel, onChange }: {
  label: string;
  ariaLabel: string;
  options: { value: string; label: string }[];
  values: string[];
  disabled?: boolean;
  emptyLabel: string;
  onChange: (values: string[]) => void;
}) {
  return (
    <Popover>
      <PopoverTrigger render={
        <Button size="sm" variant="secondary" disabled={disabled}
          className="h-7 max-w-56 gap-1 px-2 font-normal" aria-label={ariaLabel}>
          <span className="truncate">{label}</span><ChevronDown className="size-3" />
        </Button>
      } />
      <PopoverContent align="start" className="max-h-72 w-72 overflow-auto">
        {options.length === 0 ? <p className="text-caption text-muted-foreground">{emptyLabel}</p> : (
          <ul className="space-y-1.5">
            {options.map((option) => (
              <li key={option.value} className="flex items-center gap-2">
                <Checkbox checked={values.includes(option.value)} aria-label={option.label}
                  onCheckedChange={(checked) => {
                    const next = new Set(values);
                    if (checked === true) next.add(option.value); else next.delete(option.value);
                    onChange([...next]);
                  }} />
                <span className="min-w-0 truncate text-body">{option.label}</span>
              </li>
            ))}
          </ul>
        )}
      </PopoverContent>
    </Popover>
  );
}

type TriggerConfigSetter = Dispatch<SetStateAction<AutomationTriggerConfig>>;

function SlackChannelSelect({ value, options, disabled, onChange }: {
  value: string;
  options: { value: string; label: string }[];
  disabled?: boolean;
  onChange: TriggerConfigSetter;
}) {
  const { t } = useT("automations");
  const items = [{ value: "__select__", label: t(($) => $.settings.select_channel) }, ...options];
  return <InlineConfigSelect ariaLabel={t(($) => $.settings.config_channel)}
    value={value || "__select__"} items={items} disabled={disabled}
    onChange={(next) => {
      if (next === "__select__") return;
      const [installation_id, channel] = next.split("|", 2);
      onChange((prev) => ({ ...prev, installation_id, channel }));
    }} />;
}

function SlackMessageConditions({ config, canWrite, connected, channelOptions, channelValue, catalogPending, catalogError, onChange }: {
  config: AutomationTriggerConfig;
  canWrite: boolean;
  connected: boolean;
  channelOptions: { value: string; label: string }[];
  channelValue: string;
  catalogPending: boolean;
  catalogError: boolean;
  onChange: TriggerConfigSetter;
}) {
  const { t } = useT("automations");
  const filter = config.regex ?? config.keyword ?? "";
  const useRegex = config.regex !== undefined;
  return <>
    <Popover>
      <PopoverTrigger render={
        <Button size="sm" variant="secondary" disabled={!canWrite} className="h-7 max-w-56 gap-1 px-2 font-normal">
          <span className="truncate">{filter || t(($) => $.settings.any_message)}</span><ChevronDown className="size-3" />
        </Button>
      } />
      <PopoverContent align="start" className="w-80">
        <label className="space-y-1">
          <span className="text-caption text-muted-foreground">{t(($) => $.settings.config_keyword)}</span>
          <Input value={filter} placeholder={t(($) => $.settings.keyword_placeholder)}
            onChange={(event) => onChange((prev) => ({ ...prev,
              keyword: useRegex ? undefined : event.target.value,
              regex: useRegex ? event.target.value : undefined,
            }))} />
        </label>
        <label className="flex items-center gap-2 text-body">
          <Checkbox checked={useRegex} onCheckedChange={(checked) => onChange((prev) => {
            const current = prev.regex ?? prev.keyword ?? "";
            return { ...prev, keyword: checked === true ? undefined : current, regex: checked === true ? current : undefined };
          })} />
          {t(($) => $.settings.use_regex)}
        </label>
        <label className="flex items-center gap-2 text-body">
          <Checkbox checked={config.ignore_thread_replies !== false}
            onCheckedChange={(checked) => onChange((prev) => ({ ...prev, ignore_thread_replies: checked === true }))} />
          {t(($) => $.settings.ignore_thread_replies)}
        </label>
      </PopoverContent>
    </Popover>
    <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_from)}</span>
    <InlineConfigSelect ariaLabel={t(($) => $.settings.config_sender)}
      value={config.sender_scope ?? "anyone"}
      items={[
        { value: "anyone", label: t(($) => $.settings.anyone) },
        { value: "authenticated", label: t(($) => $.settings.authenticated_users) },
      ]}
      disabled={!canWrite}
      onChange={(sender_scope) => onChange((prev) => ({ ...prev,
        sender_scope: sender_scope as AutomationTriggerConfig["sender_scope"],
      }))} />
    <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_in)}</span>
    <SlackChannelSelect value={channelValue} options={channelOptions}
      disabled={!canWrite || !connected || catalogPending || catalogError} onChange={onChange} />
    <span className="text-body text-muted-foreground">; {t(($) => $.settings.react_with)}</span>
    <CompletionReactionControl value={config.completion_reaction ?? "none"}
      disabled={!canWrite}
      onChange={(completion_reaction) => onChange((prev) => ({ ...prev, completion_reaction }))} />
    <span className="text-body text-muted-foreground">{t(($) => $.settings.upon_completion)}</span>
  </>;
}

function CompletionReactionControl({ value, disabled, onChange }: {
  value: string;
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  const { t } = useT("automations");
  const [custom, setCustom] = useState(value !== "none" && value !== "white_check_mark" ? value : "");
  return <Popover>
    <PopoverTrigger render={
      <Button size="sm" variant="secondary" disabled={disabled} className="h-7 gap-1 px-2 font-normal">
        {value === "none" ? t(($) => $.settings.no_emoji) : value === "white_check_mark" ? "✅" : `:${value}:`}
        <ChevronDown className="size-3" />
      </Button>
    } />
    <PopoverContent align="start" className="w-64">
      <Button size="sm" variant="ghost" className="justify-start" onClick={() => onChange("none")}>{t(($) => $.settings.no_emoji)}</Button>
      <Button size="sm" variant="ghost" className="justify-start" onClick={() => onChange("white_check_mark")}>✅</Button>
      <label className="space-y-1">
        <span className="text-caption text-muted-foreground">{t(($) => $.settings.custom_emoji)}</span>
        <Input value={custom} placeholder={t(($) => $.settings.custom_emoji_placeholder)}
          onChange={(event) => setCustom(event.target.value)}
          onBlur={() => { const emoji = custom.trim().replaceAll(":", ""); if (emoji) onChange(emoji); }} />
      </label>
    </PopoverContent>
  </Popover>;
}

function EmojiCondition({ value, disabled, onChange }: { value: string; disabled?: boolean; onChange: (value: string) => void }) {
  const { t } = useT("automations");
  return <Popover>
    <PopoverTrigger render={
      <Button size="sm" variant="secondary" disabled={disabled} className="h-7 gap-1 px-2 font-normal">
        {value ? `:${value.replaceAll(":", "")}:` : t(($) => $.settings.any_emoji)}<ChevronDown className="size-3" />
      </Button>
    } />
    <PopoverContent align="start" className="w-64">
      <Button size="sm" variant="ghost" className="justify-start" onClick={() => onChange("")}>{t(($) => $.settings.any_emoji)}</Button>
      <Input value={value} aria-label={t(($) => $.settings.config_emoji)} placeholder={t(($) => $.settings.custom_emoji_placeholder)}
        onChange={(event) => onChange(event.target.value.replaceAll(":", ""))} />
    </PopoverContent>
  </Popover>;
}

function BranchCondition({ value, disabled, onChange }: { value: string; disabled?: boolean; onChange: (value: string) => void }) {
  const { t } = useT("automations");
  return <Popover>
    <PopoverTrigger render={
      <Button size="sm" variant="secondary" disabled={disabled} className="h-7 max-w-48 gap-1 px-2 font-normal">
        <span className="truncate">{value || t(($) => $.settings.select_branch)}</span><ChevronDown className="size-3" />
      </Button>
    } />
    <PopoverContent align="start" className="w-64">
      <Input value={value} aria-label={t(($) => $.settings.config_branch)}
        placeholder={t(($) => $.settings.select_branch)} onChange={(event) => onChange(event.target.value)} />
    </PopoverContent>
  </Popover>;
}

function LinearConditions({ preset, config, canWrite, teams, projects, states, onChange }: {
  preset: string;
  config: AutomationTriggerConfig;
  canWrite: boolean;
  teams: { id: string; name: string }[];
  projects: { id: string; name: string }[];
  states: { id: string; name: string }[];
  onChange: TriggerConfigSetter;
}) {
  const { t } = useT("automations");
  const teamItems = [{ value: "any", label: t(($) => $.settings.any_team) }, ...teams.map((team) => ({ value: team.id, label: team.name }))];
  return <>
    <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_in)}</span>
    <InlineConfigSelect ariaLabel={t(($) => $.settings.config_team)} value={config.team_id ?? "any"}
      items={teamItems} disabled={!canWrite}
      onChange={(team) => onChange((prev) => ({ ...prev, team_id: team === "any" ? undefined : team, project_id: undefined, status_id: undefined }))} />
    {preset !== "linear.cycle.ended" && <>
      <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_in)}</span>
      <InlineConfigSelect ariaLabel={t(($) => $.settings.config_project)} value={config.project_id ?? "any"}
        items={[{ value: "any", label: t(($) => $.settings.any_project) }, ...projects.map((project) => ({ value: project.id, label: project.name }))]}
        disabled={!canWrite}
        onChange={(project) => onChange((prev) => ({ ...prev, project_id: project === "any" ? undefined : project }))} />
    </>}
    {preset === "linear.issue.status_changed" && <>
      <span className="text-body text-muted-foreground">{t(($) => $.settings.condition_to)}</span>
      <InlineConfigSelect ariaLabel={t(($) => $.settings.config_status)} value={config.status_id ?? "any"}
        items={[{ value: "any", label: t(($) => $.settings.any_status) }, ...states.map((state) => ({ value: state.id, label: state.name }))]}
        disabled={!canWrite}
        onChange={(status) => onChange((prev) => ({ ...prev, status_id: status === "any" ? undefined : status }))} />
    </>}
  </>;
}
