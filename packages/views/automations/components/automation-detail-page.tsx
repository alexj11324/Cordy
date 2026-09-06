"use client";

import { useMemo, useRef, useState } from "react";
import {
  Play, Clock, Trash2, CheckCircle2, XCircle, Loader2, Pencil,
  Ban, ChevronDown, ChevronRight,
  Server,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { automationDetailOptions, automationRunsOptions, automationRunOptions } from "@patchbay/core/automations/queries";
import { projectDetailOptions } from "@patchbay/core/projects/queries";
import { agentListOptions, teamListOptions } from "@patchbay/core/workspace/queries";
import { isAgentRuntimeBound } from "@patchbay/core/agents";
import type { AutomationTriggerPreset } from "@patchbay/core/automations";
import {
  useUpdateAutomation,
  useDeleteAutomation,
  useTriggerAutomation,
  useCreateAutomationTrigger,
} from "@patchbay/core/automations/mutations";
import { clientErrorMessage, dispatchReasonCode } from "@patchbay/core/api";
import { runtimeListOptions } from "@patchbay/core/runtimes";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useActorName } from "@patchbay/core/workspace/hooks";
import { useNavigation, AppLink } from "../../navigation";
import { BreadcrumbHeader } from "../../layout/breadcrumb-header";
import { ActorAvatar } from "../../common/actor-avatar";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { Button } from "@patchbay/ui/components/ui/button";
import { Switch } from "@patchbay/ui/components/ui/switch";
import { cn } from "@patchbay/ui/lib/utils";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
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
import { ScheduleEditor } from "./schedule-editor/schedule-editor";
import { getDefaultScheduleConfig, type ScheduleConfig } from "./schedule-editor/model";
import { browserTimezone } from "../../common/timezone-select";
import { toCron } from "./schedule-editor/cron-mapping";
import { formatInTimeZone } from "../../common/format-in-time-zone";
import { SegmentedToggle } from "../../common/segmented-toggle";
import { useScheduleSubmitGate } from "./schedule-editor/validate";
import type {
  Automation,
  AutomationExecutionMode,
  AutomationRun,
  AutomationSubscriber,
} from "@patchbay/core/types";
import type { AgentTask } from "@patchbay/core/types/agent";
import { ContentEditor, ReadonlyContent } from "../../editor";
import { TranscriptButton } from "../../common/task-transcript";
import { AutomationDialog } from "./automation-dialog";
import { runNowToastKind, runNowBlockedKey } from "./run-now-toast";
import { WebhookPayloadPreview } from "./webhook-payload-preview";
import { WebhookDeliveriesSection } from "./webhook-deliveries-section";
import { ProjectIcon } from "../../projects/components/project-icon";
import { useT } from "../../i18n";
import { PageHeader } from "../../layout/page-header";
import { ModelDropdown } from "../../agents/components/model-dropdown";
import { TriggerAddMenu } from "./trigger-add-menu";
import { TriggerCard } from "./trigger-card";
import { AutomationToolsSection } from "./automation-tools-section";

// A run that already happened is an instant in the reader's day, so it reads in
// the reader's zone (no timeZone passed). A run that is still to come belongs to
// the schedule that will fire it — see the trigger row, which passes the
// trigger's own timezone.
type RunStatus = "issue_created" | "running" | "skipped" | "completed" | "failed";

const RUN_VISUAL: Record<RunStatus, { color: string; icon: typeof CheckCircle2; spin?: boolean }> = {
  issue_created: { color: "text-blue-500", icon: Clock },
  running: { color: "text-blue-500", icon: Loader2, spin: true },
  // `skipped` (admission check found the assignee runtime offline,
  // MUL-1899) is muted so it doesn't read as a failure-ratio inflator.
  // The row still shows failure_reason which carries the skip context.
  skipped: { color: "text-muted-foreground", icon: Ban },
  completed: { color: "text-emerald-500", icon: CheckCircle2 },
  failed: { color: "text-destructive", icon: XCircle },
};

// WebhookPayloadSlot lazy-fetches the full run (incl. trigger_payload) once
// the parent dialog actually mounts this slot. The list endpoint omits
// trigger_payload to keep responses small (worst case 256 KiB × N runs),
// so the detail-on-demand fetch lives here.
function WebhookPayloadSlot({ automationId, runId }: { automationId: string; runId: string }) {
  const wsId = useWorkspaceId();
  const { data, isLoading } = useQuery(
    automationRunOptions(wsId, automationId, runId),
  );
  if (isLoading) {
    return <Skeleton className="h-9 w-full" />;
  }
  if (!data || data.trigger_payload == null) {
    return null;
  }
  return <WebhookPayloadPreview payload={data.trigger_payload} />;
}

function RunRow({ run, agentId, agentName }: { run: AutomationRun; agentId: string; agentName: string }) {
  const { t, i18n } = useT("automations");
  const wsPaths = useWorkspacePaths();
  const status = (RUN_VISUAL[run.status as RunStatus] ? (run.status as RunStatus) : "issue_created");
  const visual = RUN_VISUAL[status];
  const StatusIcon = visual.icon;

  // For runs with a task_id (run_only mode), build a minimal AgentTask so
  // TranscriptButton can lazy-load the execution transcript.
  const syntheticTask: AgentTask | null = run.task_id
    ? {
        id: run.task_id,
        agent_id: agentId,
        runtime_id: "",
        issue_id: "",
        status:
          run.status === "running" ? "running" :
          run.status === "completed" ? "completed" :
          run.status === "failed" ? "failed" :
          "queued",
        priority: 0,
        dispatched_at: null,
        started_at: run.triggered_at || null,
        completed_at: run.completed_at || null,
        result: null,
        error: run.failure_reason || null,
        created_at: run.created_at,
      }
    : null;

  const content = (
    <>
      <StatusIcon className={cn("h-4 w-4 shrink-0", visual.color, visual.spin && "animate-spin")} />
      <span className={cn("w-24 shrink-0 text-caption font-medium", visual.color)}>
        {t(($) => $.run_status[status])}
      </span>
      <span className="w-20 shrink-0 text-caption text-muted-foreground">
        {t(($) => $.run_source[run.source as "schedule" | "manual" | "webhook" | "api"]) ?? run.source}
      </span>
      <span className="flex-1 min-w-0 text-caption text-muted-foreground truncate">
        {run.issue_id ? (
          t(($) => $.run.issue_linked)
        ) : run.failure_reason ? (
          <span className="text-destructive">{run.failure_reason}</span>
        ) : null}
      </span>
      <span className="w-32 shrink-0 text-right text-caption text-muted-foreground tabular-nums">
        {formatInTimeZone(run.triggered_at || run.created_at, undefined, i18n.language)}
      </span>
      {syntheticTask && !run.issue_id && (
        <TranscriptButton
          task={syntheticTask}
          agentName={agentName}
          isLive={run.status === "running"}
          title={t(($) => $.run.view_log)}
          headerSlot={
            run.source === "webhook" ? (
              <WebhookPayloadSlot automationId={run.automation_id} runId={run.id} />
            ) : undefined
          }
        />
      )}
    </>
  );

  const rowClass = "flex items-center gap-3 px-4 py-2.5 text-body hover:bg-accent/30 transition-colors";

  if (run.issue_id) {
    return (
      <AppLink href={wsPaths.issueDetail(run.issue_id)} className={cn(rowClass, "cursor-pointer")}>
        {content}
      </AppLink>
    );
  }

  return <div className={rowClass}>{content}</div>;
}

function RunHistoryList({
  runs,
  agentId,
  agentName,
}: {
  runs: AutomationRun[];
  agentId: string;
  agentName: string;
}) {
  const visibleRuns = runs.filter((run) => run.status !== "skipped");
  const skippedRuns = runs.filter((run) => run.status === "skipped");

  return (
    <div className="rounded-md border overflow-hidden">
      {visibleRuns.map((run) => (
        <RunRow key={run.id} run={run} agentId={agentId} agentName={agentName} />
      ))}
      {skippedRuns.length > 0 && (
        <SkippedRunsGroup runs={skippedRuns} agentId={agentId} agentName={agentName} />
      )}
    </div>
  );
}

function SkippedRunsGroup({
  runs,
  agentId,
  agentName,
}: {
  runs: AutomationRun[];
  agentId: string;
  agentName: string;
}) {
  const { t, i18n } = useT("automations");
  const [open, setOpen] = useState(false);
  const latestRun = runs[0];
  const ToggleIcon = open ? ChevronDown : ChevronRight;

  return (
    <div className="border-t bg-muted/20">
      <button
        type="button"
        className="flex w-full items-center gap-3 px-4 py-2.5 text-left text-body hover:bg-accent/30 transition-colors"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <ToggleIcon className="h-4 w-4 shrink-0 text-muted-foreground" />
        <Ban className="h-4 w-4 shrink-0 text-muted-foreground" />
        <span className="w-24 shrink-0 text-caption font-medium text-muted-foreground">
          {t(($) => $.run.skipped_group.label)}
        </span>
        <span className="flex-1 min-w-0 text-caption text-muted-foreground truncate">
          {t(($) => $.run.skipped_group.summary, { count: runs.length })}
        </span>
        {latestRun && (
          <span className="w-32 shrink-0 text-right text-caption text-muted-foreground tabular-nums">
            {formatInTimeZone(latestRun.triggered_at || latestRun.created_at, undefined, i18n.language)}
          </span>
        )}
      </button>
      {open && (
        <div className="border-t bg-background">
          {runs.map((run) => (
            <RunRow key={run.id} run={run} agentId={agentId} agentName={agentName} />
          ))}
        </div>
      )}
    </div>
  );
}

function AddTriggerDialog({
  open,
  onOpenChange,
  automationId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  automationId: string;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const createTrigger = useCreateAutomationTrigger();
  const [config, setConfig] = useState<ScheduleConfig>(() =>
    getDefaultScheduleConfig(browserTimezone()),
  );
  const [submitting, setSubmitting] = useState(false);
  const scheduleGate = useScheduleSubmitGate(wsId);
  const canSubmit = !submitting && scheduleGate.scheduleValid;

  const handleSubmit = async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    try {
      if (!(await scheduleGate.ensureAccepted(config))) {
        setSubmitting(false);
        return;
      }
      const cronExpr = toCron(config);
      if (!cronExpr.trim()) {
        setSubmitting(false);
        return;
      }
      await createTrigger.mutateAsync({
        automationId,
        kind: "schedule",
        cron_expression: cronExpr,
        timezone: config.timezone || undefined,
      });
      toast.success(t(($) => $.add_trigger_dialog.toast_added_schedule));
      onOpenChange(false);
      setConfig(getDefaultScheduleConfig(browserTimezone()));
    } catch (err) {
      toast.error(
        err instanceof Error && err.message
          ? err.message
          : t(($) => $.add_trigger_dialog.toast_add_failed),
      );
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogTitle>{t(($) => $.presets.scheduled)}</DialogTitle>
        <div className="min-w-0 space-y-4 pt-2">
          <ScheduleEditor
            value={config}
            onChange={(next) => {
              scheduleGate.clearRejection();
              setConfig(next);
            }}
            wsId={wsId}
            onValidityChange={scheduleGate.onValidityChange}
            disabled={submitting}
          />
          <div className="flex justify-end pt-1">
            <Button size="sm" onClick={handleSubmit} disabled={!canSubmit}>
              {submitting
                ? t(($) => $.add_trigger_dialog.submitting)
                : t(($) => $.add_trigger_dialog.submit)}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function InstructionsSection({
  automation,
  canWrite,
  runtimeId,
  thinkingLevel,
  serviceTier,
}: {
  automation: Automation;
  canWrite: boolean;
  runtimeId: string | null;
  thinkingLevel: string;
  serviceTier: string;
}) {
  const { t } = useT("automations");
  const updateAutomation = useUpdateAutomation();
  const wsId = useWorkspaceId();
  const { data: runtimes = [] } = useQuery(runtimeListOptions(wsId));
  const runtime = runtimes.find((item) => item.id === runtimeId);
  const lastSaved = useRef(automation.description ?? "");

  const persistDescription = (next: string) => {
    if (!canWrite || next === lastSaved.current) return;
    lastSaved.current = next;
    updateAutomation.mutate(
      { id: automation.id, description: next || null },
      {
        onError: (err) => {
          lastSaved.current = automation.description ?? "";
          toast.error(
            err instanceof Error ? err.message : t(($) => $.dialog.toast_update_failed),
          );
        },
      },
    );
  };

  return (
    <section className="space-y-3">
      <h2 className="text-body font-medium text-muted-foreground uppercase tracking-wider">
        {t(($) => $.settings.section_instructions)}
      </h2>
      <div className="rounded-lg border bg-background">
        {canWrite ? (
          <div className="min-h-[200px] px-4 py-3">
            <ContentEditor
              value={automation.description ?? ""}
              placeholder={t(($) => $.dialog.description_placeholder)}
              onUpdate={persistDescription}
              debounceMs={1200}
              flushPendingOnUnmount
              showBubbleMenu={false}
            />
          </div>
        ) : automation.description ? (
          <div className="px-4 py-3">
            <ReadonlyContent content={automation.description} />
          </div>
        ) : (
          <p className="px-4 py-3 text-body text-muted-foreground">
            {t(($) => $.dialog.description_placeholder)}
          </p>
        )}
        <div className="space-y-3 border-t px-4 py-3">
          <div>
            <p className="text-caption font-medium">{t(($) => $.settings.section_model)}</p>
            <p className="text-caption text-muted-foreground">
              {t(($) => $.settings.model_default)}
            </p>
          </div>
          <div>
            <ModelDropdown
              inline
              allowEffort={false}
              provider={runtime?.provider}
              thinkingLevel={thinkingLevel}
              serviceTier={serviceTier}
              runtimes={runtime ? [runtime] : []}
              runtimeId={runtimeId}
              runtimeOnline={runtime?.status === "online"}
              value={automation.model ?? ""}
              onChange={(model) => {
                if (!canWrite) return;
                return updateAutomation.mutateAsync({ id: automation.id, model: model || "" }).then(() => {});
              }}
              disabled={!canWrite}
            />
          </div>
        </div>
      </div>
    </section>
  );
}

// Read-only chip row; edits flow through AutomationDialog → SubscriberMultiSelect
// so the detail page never holds in-flight selection state.
function SubscriberChips({
  subscribers,
}: {
  subscribers: AutomationSubscriber[] | undefined;
}) {
  const { t } = useT("automations");
  const { getActorName } = useActorName();
  const members = (subscribers ?? []).filter((s) => s.user_type === "member");
  if (members.length === 0) {
    return (
      <div className="mt-1 text-body text-muted-foreground">
        {t(($) => $.detail.field_subscribers_none)}
      </div>
    );
  }
  return (
    <div className="mt-1 flex flex-wrap gap-1.5">
      {members.map((s) => (
        <span
          key={`${s.user_type}:${s.user_id}`}
          className="inline-flex items-center gap-1 rounded-full border bg-background px-2 py-0.5 text-caption"
        >
          <ActorAvatar actorType="member" actorId={s.user_id} size="xs" />
          <span className="max-w-[14rem] truncate">
            {getActorName("member", s.user_id)}
          </span>
        </span>
      ))}
    </div>
  );
}

export function AutomationDetailPage({ automationId }: { automationId: string }) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const router = useNavigation();
  const { getActorName } = useActorName();

  const { data, isLoading } = useQuery(automationDetailOptions(wsId, automationId));
  const { data: runs = [], isLoading: runsLoading } = useQuery(automationRunsOptions(wsId, automationId));
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: teams = [] } = useQuery(teamListOptions(wsId));
  const updateAutomation = useUpdateAutomation();
  const deleteAutomation = useDeleteAutomation();
  const triggerAutomation = useTriggerAutomation();
  const createTrigger = useCreateAutomationTrigger();
  const projectId = data?.automation.project_id ?? null;
  const { data: project, isLoading: projectLoading } = useQuery({
    ...projectDetailOptions(wsId, projectId ?? ""),
    enabled: Boolean(projectId),
  });

  const [detailTab, setDetailTab] = useState<"settings" | "runs">("settings");
  const [triggerDialogOpen, setTriggerDialogOpen] = useState(false);
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const executorAgentId =
    data?.automation.executor_type === "team"
      ? (teams.find((team) => team.id === data.automation.executor_id)?.leader_id ?? null)
      : (data?.automation.executor_id ?? null);
  const executorAgent = useMemo(
    () => (executorAgentId ? agents.find((agent) => agent.id === executorAgentId) ?? null : null),
    [agents, executorAgentId],
  );
  const runtimeId =
    executorAgent && isAgentRuntimeBound(executorAgent) ? executorAgent.runtime_id : null;

  if (isLoading) {
    return (
      <div className="flex h-full flex-col">
        <PageHeader>
          <Skeleton className="h-4 w-4" />
          <span className="text-muted-foreground">/</span>
          <Skeleton className="h-4 w-32" />
        </PageHeader>
        <div className="flex-1 overflow-y-auto">
          <div className="max-w-4xl mx-auto p-6 space-y-8">
            <section className="space-y-4">
              <Skeleton className="h-3 w-20" />
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-1">
                  <Skeleton className="h-3 w-12" />
                  <Skeleton className="h-5 w-32" />
                </div>
                <div className="space-y-1">
                  <Skeleton className="h-3 w-12" />
                  <Skeleton className="h-5 w-24" />
                </div>
              </div>
            </section>
            <section className="space-y-3">
              <Skeleton className="h-4 w-16" />
              <Skeleton className="h-10 w-full rounded-md" />
            </section>
            <section className="space-y-3">
              <Skeleton className="h-4 w-24" />
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-10 w-full" />
              ))}
            </section>
          </div>
        </div>
      </div>
    );
  }

  if (!data) {
    return (
      <div className="flex items-center justify-center h-full text-muted-foreground">
        {t(($) => $.detail.not_found)}
      </div>
    );
  }

  const { automation, triggers } = data;
  const collaborators = data.collaborators ?? [];
  // Treat an absent can_write (older server) as "allowed" — the backend is the
  // real gate, so the UI only hides controls when the server explicitly says
  // the caller cannot write.
  const canWrite = automation.can_write !== false;
  // Managing the access list is narrower than write: granted collaborators can
  // edit/run but cannot grant/revoke. Fall back to canWrite when the server
  // doesn't send the field (older backend).
  const canManageAccess = automation.can_manage_access ?? canWrite;

  const handleRunNow = async () => {
    try {
      const run = await triggerAutomation.mutateAsync(automationId);
      // Manual "run now" returns 200 even when admission blocks the run, so the
      // toast is driven by the run's domain status, not the HTTP 2xx (MUL-4525).
      // Success is a whitelist (issue_created/running) — a skipped run warns, a
      // failed or unknown/future status errors — never a false "triggered".
      const kind = runNowToastKind(run?.status);
      if (kind === "success") {
        toast.success(t(($) => $.detail.toast_triggered));
        return;
      }
      // reason_code is the stable, typed cause the server decided at admission
      // time; an unknown/absent code degrades to a generic "not triggered".
      const message = t(($) => $.detail[runNowBlockedKey(run?.reason_code)]);
      if (kind === "warning") {
        toast.warning(message);
      } else {
        toast.error(message);
      }
    } catch (e: any) {
      const reason = dispatchReasonCode(e);
      if (reason) {
        toast.error(t(($) => $.detail[runNowBlockedKey(reason)]));
        return;
      }
      // Only a 4xx message is written for the user; a 5xx one is internal
      // server detail (MUL-6472), so an unclassified dispatch failure shows the
      // localized generic sentence instead of the raw body.
      toast.error(clientErrorMessage(e) || t(($) => $.detail.toast_trigger_failed));
    }
  };

  const handleDelete = async () => {
    setDeleting(true);
    try {
      await deleteAutomation.mutateAsync(automationId);
      toast.success(t(($) => $.detail.toast_deleted));
      router.push(wsPaths.automations());
    } catch (err) {
      toast.error(
        err instanceof Error && err.message
          ? err.message
          : t(($) => $.detail.toast_delete_failed),
      );
      setDeleting(false);
    }
  };

  const handleToggleStatus = (checked: boolean) => {
    updateAutomation.mutate({ id: automationId, status: checked ? "active" : "paused" });
  };

  const handlePickPreset = async (preset: AutomationTriggerPreset) => {
    try {
      await createTrigger.mutateAsync({
        automationId,
        kind: preset.kind,
        preset: preset.id,
      });
      toast.success(t(($) => $.settings.toast_trigger_added));
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : t(($) => $.settings.toast_trigger_add_failed),
      );
    }
  };

  const hasGenericWebhook = triggers.some(
    (trig) => trig.kind === "webhook" && Boolean(trig.webhook_token),
  );

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <BreadcrumbHeader
        segments={[{ href: wsPaths.automations(), label: t(($) => $.page.title) }]}
        leaf={
          <>
            <h1 className="min-w-0 truncate text-body font-medium text-foreground">{automation.title}</h1>
            <div className="ml-1 flex items-center gap-1.5 shrink-0">
              <Switch
                size="sm"
                checked={automation.status === "active"}
                onCheckedChange={handleToggleStatus}
                disabled={automation.status === "archived"}
                aria-label={
                  automation.status === "active"
                    ? t(($) => $.detail.pause_aria)
                    : t(($) => $.detail.activate_aria)
                }
              />
              <span className={cn(
                "text-caption font-medium hidden sm:inline",
                automation.status === "active" ? "text-emerald-500" :
                automation.status === "paused" ? "text-amber-500" :
                "text-muted-foreground",
              )}>
                {t(($) => $.status[automation.status])}
              </span>
            </div>
          </>
        }
        actions={
          canWrite ? (
            <>
              <Button size="sm" variant="outline" onClick={() => setEditDialogOpen(true)} className="px-2 sm:px-2.5" aria-label={t(($) => $.detail.edit)}>
                <Pencil className="h-3.5 w-3.5 sm:mr-1" />
                <span className="hidden sm:inline">{t(($) => $.detail.edit)}</span>
              </Button>
              <Button
                size="sm"
                onClick={handleRunNow}
                disabled={automation.status !== "active" || triggerAutomation.isPending}
                className="px-2 sm:px-2.5"
                aria-label={triggerAutomation.isPending ? t(($) => $.detail.running) : t(($) => $.detail.run_now)}
              >
                {triggerAutomation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 sm:mr-1 animate-spin" />
                ) : (
                  <Play className="h-3.5 w-3.5 sm:mr-1" />
                )}
                <span className="hidden sm:inline">
                  {triggerAutomation.isPending
                    ? t(($) => $.detail.running)
                    : t(($) => $.detail.run_now)}
                </span>
              </Button>
            </>
          ) : null
        }
      />

      {automation.pause_reason === "agent_runtime_required" && (
        <div className="flex shrink-0 items-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-6 py-2 text-caption text-amber-900 dark:text-amber-100">
          <Server className="size-3.5 shrink-0" />
          <span className="flex-1">
            {t(($) => $.detail.paused_runtime_required)}
          </span>
          {automation.executor_type === "agent" && (
            <AppLink
              href={`${wsPaths.agentDetail(automation.executor_id)}?view=general`}
              className="font-medium underline underline-offset-2"
            >
              {t(($) => $.detail.bind_runtime)}
            </AppLink>
          )}
        </div>
      )}

      <div className="flex shrink-0 items-center border-b px-6 py-2">
        <div className="w-64">
          <SegmentedToggle
            value={detailTab}
            onChange={setDetailTab}
            options={[
              ["settings", t(($) => $.settings.tab_settings)],
              ["runs", t(($) => $.settings.tab_runs)],
            ]}
          />
        </div>
      </div>

      <div className="flex-1 overflow-y-auto">
        <div className="max-w-4xl mx-auto p-6 space-y-8">
          {detailTab === "settings" ? (
            <>
          {/* Properties */}
          <section className="space-y-4">
            <h2 className="text-body font-medium text-muted-foreground uppercase tracking-wider">
              {t(($) => $.detail.section_properties)}
            </h2>
            <div className="grid grid-cols-2 gap-4 text-body">
              <div>
                <label className="text-caption text-muted-foreground">{t(($) => $.detail.field_agent)}</label>
                <div className="mt-1 flex items-center gap-2">
                  <ActorAvatar
                    actorType={automation.executor_type}
                    actorId={automation.executor_id}
                    size="sm"
                    enableHoverCard={automation.executor_type === "agent"}
                    showStatusDot={automation.executor_type === "agent"}
                  />
                  <span className="cursor-pointer">
                    {getActorName(automation.executor_type, automation.executor_id)}
                  </span>
                </div>
              </div>
              <div>
                <label className="text-caption text-muted-foreground">{t(($) => $.detail.field_created_by)}</label>
                <div className="mt-1 flex items-center gap-2">
                  {/* Creator may be a member or an agent: the HTTP create path stamps
                      member today, but backend logic also writes created_by_type=agent.
                      ActorAvatar/getActorName resolve both, so never assume member. */}
                  <ActorAvatar
                    actorType={automation.created_by_type}
                    actorId={automation.created_by_id}
                    size="sm"
                    enableHoverCard
                    showStatusDot={automation.created_by_type === "agent"}
                  />
                  <span className="cursor-pointer">
                    {getActorName(automation.created_by_type, automation.created_by_id)}
                  </span>
                </div>
              </div>
              <div>
                <label className="text-caption text-muted-foreground">{t(($) => $.detail.field_output_mode)}</label>
                <div className="mt-1">
                  {t(($) => $.execution_mode[automation.execution_mode as AutomationExecutionMode])}
                </div>
              </div>
              {/* Shown for BOTH output modes (MUL-6681): a run_only automation's
                  project decides its execution environment (repository /
                  local_directory, and therefore worktree isolation), so an
                  operator debugging a run needs to see it here. */}
              <div>
                <label className="text-caption text-muted-foreground">{t(($) => $.detail.field_project)}</label>
                <div className="mt-1 min-w-0">
                  {!automation.project_id ? (
                    <span className="text-muted-foreground">{t(($) => $.detail.no_project)}</span>
                  ) : projectLoading ? (
                    <Skeleton className="h-5 w-32" />
                  ) : project ? (
                    <AppLink
                      href={wsPaths.projectDetail(project.id)}
                      className="inline-flex max-w-full items-center gap-1.5 text-foreground hover:underline"
                    >
                      <ProjectIcon project={project} size="md" />
                      <span className="truncate">{project.title}</span>
                    </AppLink>
                  ) : (
                    <span className="text-muted-foreground">{t(($) => $.detail.project_unavailable)}</span>
                  )}
                </div>
              </div>
              {automation.execution_mode === "create_issue" && (
                <div className="col-span-2">
                  <label className="text-caption text-muted-foreground">
                    {t(($) => $.detail.field_subscribers)}
                  </label>
                  <SubscriberChips
                    subscribers={automation.subscribers}
                  />
                </div>
              )}
            </div>
          </section>

          {/* Triggers */}
          <section className="space-y-3">
            <div className="flex items-center justify-between">
              <h2 className="text-body font-medium text-muted-foreground uppercase tracking-wider">
                {t(($) => $.detail.section_triggers)}
              </h2>
              <TriggerAddMenu
                canWrite={canWrite}
                onPickSchedule={() => setTriggerDialogOpen(true)}
                onPickPreset={handlePickPreset}
              />
            </div>
            {triggers.length === 0 ? (
              <div className="rounded-md border border-dashed p-4 text-center text-body text-muted-foreground">
                {t(($) => $.detail.no_triggers)}
              </div>
            ) : (
              <div className="space-y-2">
                {triggers.map((trig) => (
                  <TriggerCard
                    key={trig.id}
                    trigger={trig}
                    automationId={automationId}
                    canWrite={canWrite}
                  />
                ))}
              </div>
            )}
          </section>

          <InstructionsSection
            automation={automation}
            canWrite={canWrite}
            runtimeId={runtimeId}
            thinkingLevel={executorAgent?.thinking_level ?? ""}
            serviceTier={executorAgent?.service_tier ?? ""}
          />

          <AutomationToolsSection automation={automation} canWrite={canWrite} />

          {/* Generic webhook deliveries only — native GitHub/Slack/Linear
              triggers have no public URL and share the platform ingress. */}
          <WebhookDeliveriesSection
            automationId={automationId}
            hasWebhookTrigger={hasGenericWebhook}
          />

          {/* Danger zone */}
          {canWrite && (
            <section className="space-y-3 pt-4 border-t">
              <h2 className="text-body font-medium text-destructive uppercase tracking-wider">
                {t(($) => $.detail.section_danger)}
              </h2>
              <Button size="sm" variant="destructive" onClick={() => setDeleteConfirmOpen(true)}>
                <Trash2 className="h-3.5 w-3.5 mr-1" />
                {t(($) => $.detail.delete_button)}
              </Button>
            </section>
          )}
            </>
          ) : (
          <section className="space-y-3">
            <h2 className="text-body font-medium text-muted-foreground uppercase tracking-wider">
              {t(($) => $.detail.section_run_history)}
            </h2>
            {runsLoading ? (
              <div className="space-y-1">
                {Array.from({ length: 3 }).map((_, i) => (
                  <Skeleton key={i} className="h-10 w-full" />
                ))}
              </div>
            ) : runs.length === 0 ? (
              <div className="rounded-md border border-dashed p-4 text-center text-body text-muted-foreground">
                {t(($) => $.detail.no_runs)}
              </div>
            ) : (
              <RunHistoryList
                runs={runs}
                agentId={automation.executor_id}
                agentName={getActorName(automation.executor_type, automation.executor_id)}
              />
            )}
          </section>
          )}
        </div>
      </div>

      {/* Mounted only while open, like the edit dialog: otherwise a rejected
          cron leaves scheduleValid=false behind for the next open. */}
      {triggerDialogOpen && (
        <AddTriggerDialog
          open={triggerDialogOpen}
          onOpenChange={setTriggerDialogOpen}
          automationId={automationId}
        />
      )}
      {editDialogOpen && (
        <AutomationDialog
          mode="edit"
          open={editDialogOpen}
          onOpenChange={setEditDialogOpen}
          automationId={automation.id}
          initial={{
            title: automation.title,
            description: automation.description ?? "",
            project_id: automation.project_id ?? null,
            executor_type: automation.executor_type,
            executor_id: automation.executor_id,
            execution_mode: automation.execution_mode as AutomationExecutionMode,
            subscriber_user_ids:
              automation.subscribers
                ?.filter((s) => s.user_type === "member")
                .map((s) => s.user_id) ?? [],
          }}
          triggers={triggers}
          collaborators={collaborators}
          canManageAccess={canManageAccess}
        />
      )}
      <AlertDialog
        open={deleteConfirmOpen}
        onOpenChange={(v) => { if (!v && !deleting) setDeleteConfirmOpen(false); }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t(($) => $.detail.delete_dialog.title)}</AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.detail.delete_dialog.description, { title: automation.title })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>
              {t(($) => $.detail.delete_dialog.cancel)}
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={handleDelete}
              disabled={deleting}
              className="bg-destructive text-white hover:bg-destructive/90"
            >
              {deleting
                ? t(($) => $.detail.delete_dialog.deleting)
                : t(($) => $.detail.delete_dialog.confirm)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
