"use client";

import { useRef, useState } from "react";
import {
  Play, Clock, Trash2, CheckCircle2, XCircle, Loader2, Pencil,
  Ban, ChevronDown, ChevronRight, FolderKanban, MoreHorizontal, Server, AlertTriangle,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { automationDetailOptions, automationRunsOptions } from "@patchbay/core/automations/queries";
import { projectDetailOptions } from "@patchbay/core/projects/queries";
import type { AutomationTriggerPreset } from "@patchbay/core/automations";
import {
  useUpdateAutomation,
  useDeleteAutomation,
  useTriggerAutomation,
  useCreateAutomationTrigger,
} from "@patchbay/core/automations/mutations";
import { clientErrorMessage, dispatchReasonCode, errorCode } from "@patchbay/core/api";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useActorName } from "@patchbay/core/workspace/hooks";
import { useNavigation, AppLink } from "../../navigation";
import { BreadcrumbHeader } from "../../layout/breadcrumb-header";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { Button } from "@patchbay/ui/components/ui/button";
import { Switch } from "@patchbay/ui/components/ui/switch";
import { cn } from "@patchbay/ui/lib/utils";
import { toast } from "sonner";
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
import { formatInTimeZone } from "../../common/format-in-time-zone";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@patchbay/ui/components/ui/tabs";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@patchbay/ui/components/ui/dropdown-menu";
import type {
  Automation,
  AutomationExecutionMode,
  AutomationRun,
} from "@patchbay/core/types";
import type { AgentTask } from "@patchbay/core/types/agent";
import { ContentEditor, ReadonlyContent } from "../../editor";
import { AgentThreadButton } from "../../agent-thread";
import { AutomationDialog } from "./automation-dialog";
import { runNowToastKind, runNowBlockedKey } from "./run-now-toast";
import { WebhookDeliveriesSection } from "./webhook-deliveries-section";
import { ProjectIcon } from "../../projects/components/project-icon";
import { ProjectPicker } from "../../projects/components/project-picker";
import { useT } from "../../i18n";
import { PageHeader } from "../../layout/page-header";
import { AgentPicker } from "./pickers/agent-picker";
import { TriggerAddMenu } from "./trigger-add-menu";
import { TriggerCard } from "./trigger-card";
import { TriggerScheduleDialog } from "./trigger-schedule-dialog";
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

function getToolDeliveryErrors(result: unknown): string[] {
  if (!result || typeof result !== "object" || Array.isArray(result)) return [];
  const errors = (result as Record<string, unknown>).tool_delivery_errors;
  if (!Array.isArray(errors)) return [];
  return errors.flatMap((error) => {
    if (!error || typeof error !== "object" || Array.isArray(error)) return [];
    const message = (error as Record<string, unknown>).message;
    return typeof message === "string" && message.trim() !== "" ? [message] : [];
  });
}

function RunRow({ run, agentId }: { run: AutomationRun; agentId: string }) {
  const { t, i18n } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const status = (RUN_VISUAL[run.status as RunStatus] ? (run.status as RunStatus) : "issue_created");
  const visual = RUN_VISUAL[status];
  const StatusIcon = visual.icon;
  const deliveryErrors = getToolDeliveryErrors(run.result);

  // Run-only executions have no issue route, so their task opens directly in
  // the same interactive Agent conversation used by issue and Agent surfaces.
  const syntheticTask: AgentTask | null = run.task_id
    ? {
        id: run.task_id,
        workspace_id: wsId,
        agent_id: agentId,
        runtime_id: "",
        issue_id: run.issue_id ?? "",
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
    </>
  );

  const rowClass = "flex items-center gap-3 px-4 py-2.5 text-body hover:bg-accent/30 transition-colors";
  const row = (
    <div className={rowClass}>
      {run.issue_id ? (
        <AppLink
          href={wsPaths.issueDetail(run.issue_id)}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-3"
        >
          {content}
        </AppLink>
      ) : content}
      {syntheticTask && (
        <AgentThreadButton
          task={syntheticTask}
          title={t(($) => $.run.view_conversation)}
        />
      )}
    </div>
  );

  return (
    <div>
      {row}
      {deliveryErrors.length > 0 && (
        <div
          role="alert"
          className="mx-4 mb-2 flex items-start gap-1.5 rounded-md bg-destructive/10 px-2.5 py-2 text-caption text-destructive"
        >
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span>
            <span className="font-medium">{t(($) => $.run.delivery_failed)}:</span>{" "}
            {deliveryErrors.join(" · ")}
          </span>
        </div>
      )}
    </div>
  );
}

function RunHistoryList({
  runs,
  agentId,
}: {
  runs: AutomationRun[];
  agentId: string;
}) {
  const visibleRuns = runs.filter((run) => run.status !== "skipped");
  const skippedRuns = runs.filter((run) => run.status === "skipped");

  return (
    <div className="rounded-md border overflow-hidden">
      {visibleRuns.map((run) => (
        <RunRow key={run.id} run={run} agentId={agentId} />
      ))}
      {skippedRuns.length > 0 && (
        <SkippedRunsGroup runs={skippedRuns} agentId={agentId} />
      )}
    </div>
  );
}

function SkippedRunsGroup({
  runs,
  agentId,
}: {
  runs: AutomationRun[];
  agentId: string;
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
            <RunRow key={run.id} run={run} agentId={agentId} />
          ))}
        </div>
      )}
    </div>
  );
}

function InstructionsSection({
  automation,
  canWrite,
}: {
  automation: Automation;
  canWrite: boolean;
}) {
  const { t } = useT("automations");
  const updateAutomation = useUpdateAutomation();
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
    <section className="space-y-2" data-testid="automation-instructions">
      <h2 className="text-caption font-medium uppercase tracking-wider text-muted-foreground">
        {t(($) => $.settings.section_instructions)}
      </h2>
      <div className="rounded-lg border bg-background">
        <div className="automation-instructions h-44 overflow-y-auto overscroll-contain px-4 py-3">
        {canWrite ? (
            <ContentEditor
              value={automation.description ?? ""}
              placeholder={t(($) => $.dialog.description_placeholder)}
              onUpdate={persistDescription}
              debounceMs={1200}
              flushPendingOnUnmount
              showBubbleMenu={false}
            />
        ) : automation.description ? (
            <ReadonlyContent content={automation.description} />
        ) : (
          <p className="text-label text-muted-foreground">
            {t(($) => $.dialog.description_placeholder)}
          </p>
        )}
        </div>
        <div className="flex min-w-0 items-center px-3 pb-2">
          <AgentPicker
            assignee={{ type: automation.executor_type, id: automation.executor_id }}
            disabled={!canWrite || updateAutomation.isPending}
            onChange={(next) => {
              if (!canWrite) return;
              updateAutomation.mutate({
                id: automation.id,
                executor_type: next.type,
                executor_id: next.id,
                // The selected Agent owns its model configuration. Empty is
                // the established API sentinel for clearing a legacy
                // automation-level override.
                model: "",
              }, {
                onError: (error) => toast.error(
                  error instanceof Error ? error.message : t(($) => $.dialog.toast_update_failed),
                ),
              });
            }}
          />
        </div>
      </div>
    </section>
  );
}

export function AutomationDetailPage({ automationId }: { automationId: string }) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const router = useNavigation();
  const { getActorName } = useActorName();

  const { data, isLoading, isError, refetch } = useQuery(automationDetailOptions(wsId, automationId));
  const { data: runs = [], isLoading: runsLoading, isError: runsError, refetch: refetchRuns } = useQuery(automationRunsOptions(wsId, automationId));
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
  const [runBlockedOpen, setRunBlockedOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);

  if (isLoading) {
    return (
      <div className="flex h-full flex-col">
        <PageHeader>
          <Skeleton className="h-4 w-4" />
          <span className="text-muted-foreground">/</span>
          <Skeleton className="h-4 w-32" />
        </PageHeader>
        <div className="flex-1 overflow-y-auto">
          <div className="mx-auto max-w-3xl px-6 py-8 space-y-8">
            <section className="space-y-3">
              <Skeleton className="h-8 w-64" />
              <Skeleton className="h-5 w-80" />
            </section>
            <section className="space-y-3">
              <Skeleton className="h-5 w-20" />
              <Skeleton className="h-24 w-full rounded-lg" />
            </section>
            <section className="space-y-3">
              <Skeleton className="h-5 w-40" />
              <Skeleton className="h-48 w-full rounded-lg" />
            </section>
          </div>
        </div>
      </div>
    );
  }

  if (!data) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-body text-muted-foreground">
        <p role="status">{isError ? t(($) => $.settings.load_failed) : t(($) => $.detail.not_found)}</p>
        {isError && <Button size="sm" variant="outline" onClick={() => void refetch()}>{t(($) => $.page.retry)}</Button>}
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
  const runReady = automation.run_ready !== false;

  const handleRunNow = async () => {
    if (!runReady) {
      setRunBlockedOpen(true);
      return;
    }
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
      if (errorCode(e) === "automation_trigger_not_ready") {
        setRunBlockedOpen(true);
        return;
      }
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
    updateAutomation.mutate({ id: automationId, status: checked ? "active" : "paused" }, {
      onError: (error) => toast.error(error instanceof Error ? error.message : t(($) => $.dialog.toast_update_failed)),
    });
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

  const hasWebhookTrigger = triggers.some((trig) => trig.kind === "webhook");

  return (
    <div className="flex h-full flex-col">
      <BreadcrumbHeader
        segments={[{ href: wsPaths.automations(), label: t(($) => $.page.title) }]}
        leaf={
          <span className="min-w-0 truncate text-caption text-muted-foreground">
            {automation.title}
          </span>
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
                variant="outline"
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
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="text-muted-foreground"
                      aria-label={t(($) => $.detail.more_actions)}
                    >
                      <MoreHorizontal />
                    </Button>
                  }
                />
                <DropdownMenuContent align="end">
                  <DropdownMenuItem
                    variant="destructive"
                    onClick={() => setDeleteConfirmOpen(true)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                    {t(($) => $.detail.delete_button)}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
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

      <Tabs
        value={detailTab}
        onValueChange={(value) => setDetailTab(value as "settings" | "runs")}
        className="flex min-h-0 flex-1 flex-col gap-0"
      >
        <div className="flex-1 overflow-y-auto">
          <div className="mx-auto max-w-3xl space-y-8 px-4 py-6 sm:px-8 sm:py-8">
            <header className="space-y-4">
              <h1
                data-testid="automation-settings-title"
                className="text-display-sm font-bold leading-snug tracking-tight text-foreground"
              >
                {automation.title}
              </h1>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                <div className="flex items-center gap-1.5">
                  <Switch
                    size="sm"
                    checked={automation.status === "active"}
                    onCheckedChange={handleToggleStatus}
                    disabled={automation.status === "archived" || !canWrite || updateAutomation.isPending}
                    aria-label={
                      automation.status === "active"
                        ? t(($) => $.detail.pause_aria)
                        : t(($) => $.detail.activate_aria)
                    }
                  />
                  <span className={cn(
                    "text-caption font-medium",
                    automation.status === "active" ? "text-emerald-500" : "text-muted-foreground",
                  )}>
                    {automation.status === "active"
                      ? t(($) => $.status.active)
                      : t(($) => $.detail.status_inactive)}
                  </span>
                </div>
                <span aria-hidden className="h-3.5 w-px shrink-0 bg-border" />
                <ProjectPicker
                  projectId={automation.project_id ?? null}
                  disabled={!canWrite || updateAutomation.isPending}
                  onUpdate={(updates) => {
                    if (!canWrite || updates.project_id === undefined) return;
                    updateAutomation.mutate({
                      id: automationId,
                      project_id: updates.project_id,
                    }, {
                      onError: (error) => toast.error(error instanceof Error ? error.message : t(($) => $.dialog.toast_update_failed)),
                    });
                  }}
                  triggerRender={
                    <button
                      type="button"
                      disabled={!canWrite || updateAutomation.isPending}
                      className="inline-flex h-7 max-w-[14rem] items-center gap-1 rounded-md px-1 text-caption text-muted-foreground hover:bg-accent/30 disabled:pointer-events-none disabled:opacity-50"
                    >
                      {projectLoading ? (
                        <Skeleton className="h-3.5 w-20" />
                      ) : project ? (
                        <>
                          <ProjectIcon project={project} size="sm" />
                          <span className="truncate text-foreground">{project.title}</span>
                        </>
                      ) : (
                        <>
                          <FolderKanban className="size-3.5 shrink-0" />
                          <span className="truncate">{t(($) => $.detail.no_project)}</span>
                        </>
                      )}
                      <ChevronDown className="size-3 shrink-0" />
                    </button>
                  }
                />
                <span aria-hidden className="h-3.5 w-px shrink-0 bg-border" />
                <span className="text-caption text-muted-foreground">
                  {t(($) => $.detail.created_by_line, {
                    name: getActorName(automation.created_by_type, automation.created_by_id),
                  })}
                </span>
              </div>
              <TabsList className="h-7 bg-transparent p-0">
                <TabsTrigger
                  value="settings"
                  className="h-7 flex-none rounded-md px-2.5 text-label after:hidden data-active:bg-muted data-active:shadow-none"
                >
                  {t(($) => $.settings.tab_settings)}
                </TabsTrigger>
                <TabsTrigger
                  value="runs"
                  className="h-7 flex-none rounded-md px-2.5 text-label after:hidden data-active:bg-muted data-active:shadow-none"
                >
                  {t(($) => $.settings.tab_runs)}
                </TabsTrigger>
              </TabsList>
            </header>

            <TabsContent value="settings" className="mt-0 space-y-8">
              <section className="space-y-2">
                <h2 className="text-caption font-medium uppercase tracking-wider text-muted-foreground">
                  {t(($) => $.detail.section_triggers)}
                </h2>
                <p className="text-caption text-muted-foreground">{t(($) => $.settings.trigger_help)}</p>
                <div
                  data-testid="automation-triggers-card"
                  className="divide-y rounded-lg border bg-background"
                >
                  {triggers.length === 0 && !canWrite ? (
                    <p className="px-4 py-6 text-center text-body text-muted-foreground">
                      {t(($) => $.detail.no_triggers)}
                    </p>
                  ) : (
                    triggers.map((trig) => (
                      <TriggerCard
                        key={trig.id}
                        trigger={trig}
                        automationId={automationId}
                        canWrite={canWrite}
                      />
                    ))
                  )}
                  <div className="px-2 py-1">
                    <TriggerAddMenu
                      canWrite={canWrite}
                      variant="inset"
                      onPickSchedule={() => setTriggerDialogOpen(true)}
                      onPickPreset={handlePickPreset}
                    />
                  </div>
                </div>
              </section>

              <InstructionsSection
                automation={automation}
                canWrite={canWrite}
              />

              <AutomationToolsSection automation={automation} canWrite={canWrite} />

              <WebhookDeliveriesSection
                automationId={automationId}
                hasWebhookTrigger={hasWebhookTrigger}
              />
            </TabsContent>

            <TabsContent value="runs" className="mt-0 space-y-3">
              {runsLoading ? (
                <div className="space-y-1">
                  {Array.from({ length: 3 }).map((_, i) => (
                    <Skeleton key={i} className="h-10 w-full" />
                  ))}
                </div>
              ) : runsError ? (
                <div className="space-y-3 rounded-md border border-dashed p-4 text-center text-body text-muted-foreground">
                  <p role="status">{t(($) => $.settings.runs_load_failed)}</p>
                  <Button size="sm" variant="outline" onClick={() => void refetchRuns()}>{t(($) => $.page.retry)}</Button>
                </div>
              ) : runs.length === 0 ? (
                <div className="rounded-md border border-dashed p-4 text-center text-body text-muted-foreground">
                  {t(($) => $.detail.no_runs)}
                </div>
              ) : (
                <RunHistoryList
                  runs={runs}
                  agentId={automation.executor_id}
                />
              )}
            </TabsContent>
          </div>
        </div>
      </Tabs>

      {/* Mounted only while open, like the edit dialog: otherwise a rejected
          cron leaves scheduleValid=false behind for the next open. */}
      {triggerDialogOpen && (
        <TriggerScheduleDialog
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
      <AlertDialog open={runBlockedOpen} onOpenChange={setRunBlockedOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t(($) => $.detail.run_blocked_dialog.title)}</AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.detail.run_blocked_dialog.description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogAction onClick={() => {
              setRunBlockedOpen(false);
              setDetailTab("settings");
            }}>
              {t(($) => $.detail.run_blocked_dialog.confirm)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
