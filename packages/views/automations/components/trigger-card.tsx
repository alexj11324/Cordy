"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Clock, RotateCw, Trash2, Webhook } from "lucide-react";
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
import { Switch } from "@patchbay/ui/components/ui/switch";
import { toast } from "sonner";
import { AppLink } from "../../navigation";
import { useDescribeSchedule } from "./schedule-editor/describe";
import { parseCron } from "./schedule-editor/cron-mapping";
import { WebhookUrlField } from "./webhook-url-field";
import { useT } from "../../i18n";
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

export function TriggerCard({
  trigger,
  automationId,
  canWrite,
}: {
  trigger: AutomationTrigger;
  automationId: string;
  canWrite: boolean;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const describeSchedule = useDescribeSchedule();
  const updateTrigger = useUpdateAutomationTrigger();
  const deleteTrigger = useDeleteAutomationTrigger();
  const rotateToken = useRotateAutomationTriggerWebhookToken();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [rotateOpen, setRotateOpen] = useState(false);
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
    <div className="rounded-lg border bg-background p-4 space-y-3">
      <div className="flex items-start gap-3">
        {trigger.kind === "webhook" ? (
          <Webhook className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        ) : (
          <Clock className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        )}
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <p className="truncate text-body font-medium">{title}</p>
            {!trigger.enabled && (
              <span className="text-caption text-muted-foreground">
                {t(($) => $.trigger_row.disabled_badge)}
              </span>
            )}
          </div>
          {scheduleDescription && (
            <p className="mt-0.5 text-caption text-muted-foreground">{scheduleDescription}</p>
          )}
        </div>
        {canWrite && (
          <div className="flex items-center gap-1 shrink-0">
            <Switch
              size="sm"
              checked={trigger.enabled}
              onCheckedChange={(checked) => {
                updateTrigger.mutate({ automationId, triggerId: trigger.id, enabled: checked });
              }}
              aria-label={title}
            />
            <Button
              size="sm"
              variant="ghost"
              className="px-2 text-muted-foreground hover:text-destructive"
              onClick={() => setConfirmOpen(true)}
              aria-label={t(($) => $.trigger_row.delete_dialog.title)}
            >
              <Trash2 className="size-3.5" />
            </Button>
          </div>
        )}
      </div>

      {native && !connected && (
        <div className="rounded-md bg-muted/50 px-3 py-2 text-caption text-muted-foreground">
          <span>
            {provider === "github"
              ? t(($) => $.settings.connect_github)
              : provider === "slack"
                ? t(($) => $.settings.connect_slack)
                : t(($) => $.settings.connect_linear)}
          </span>{" "}
          <AppLink
            href={settingsPathForTriggerProvider(wsPaths.settings(), provider)}
            className="font-medium text-foreground underline underline-offset-2"
          >
            {t(($) => $.settings.connect_cta)}
          </AppLink>
        </div>
      )}

      {webhookUrl && (
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
      )}

      {canWrite && native && (
        <div className="grid gap-2 sm:grid-cols-2">
          {trigger.preset === "slack.message" && (
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
