"use client";

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Play,
  Clock,
  Trash2,
  Loader2,
  Pencil,
  MoreHorizontal,
  Server,
  AlertTriangle,
  ListFilter,
  Search,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import {
  automationDetailOptions,
  automationRunsOptions,
} from "@orvilo/core/automations/queries";
import type { AutomationTriggerPreset } from "@orvilo/core/automations";
import {
  useUpdateAutomation,
  useDeleteAutomation,
  useTriggerAutomation,
  useCreateAutomationTrigger,
} from "@orvilo/core/automations/mutations";
import {
  clientErrorMessage,
  dispatchReasonCode,
  errorCode,
} from "@orvilo/core/api";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { useActorName } from "@orvilo/core/workspace/hooks";
import { useNavigation, AppLink } from "../../navigation";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { Button } from "@orvilo/ui/components/ui/button";
import { Badge } from "@orvilo/ui/components/ui/badge";
import { Input } from "@orvilo/ui/components/ui/input";
import { cn } from "@orvilo/ui/lib/utils";
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
} from "@orvilo/ui/components/ui/alert-dialog";
import { formatInTimeZone } from "../../common/format-in-time-zone";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@orvilo/ui/components/ui/table";
import type {
  Automation,
  AutomationExecutionMode,
  AutomationRun,
  AutomationTrigger,
} from "@orvilo/core/types";
import type { AgentTask } from "@orvilo/core/types/agent";
import { AgentThreadButton } from "../../agent-thread";
import { AutomationSettingsPage } from "./automation-settings-page";
import { AutomationEditDialog } from "./automation-edit-dialog";
import { runNowToastKind, runNowBlockedKey } from "./run-now-toast";
import { WebhookDeliveriesSection } from "./webhook-deliveries-section";
import { useT } from "../../i18n";
import { PageHeader } from "../../layout/page-header";
import {
  AutomationInstructionsCard,
  AutomationTriggersSection,
} from "./automation-settings-sections";
import { TriggerCard } from "./trigger-card";
import { TriggerScheduleDialog } from "./trigger-schedule-dialog";
import { AutomationToolsSection } from "./automation-tools-section";

// A run that already happened is an instant in the reader's day, so it reads in
// the reader's zone (no timeZone passed). A run that is still to come belongs to
// the schedule that will fire it — see the trigger row, which passes the
// trigger's own timezone.
type RunStatus =
  | "issue_created"
  | "running"
  | "skipped"
  | "completed"
  | "failed";

const RUN_STATUSES: RunStatus[] = [
  "issue_created",
  "running",
  "skipped",
  "completed",
  "failed",
];

function normalizedRunStatus(value: string): RunStatus {
  return RUN_STATUSES.includes(value as RunStatus)
    ? (value as RunStatus)
    : "issue_created";
}

function getToolDeliveryErrors(result: unknown): string[] {
  if (!result || typeof result !== "object" || Array.isArray(result)) return [];
  const errors = (result as Record<string, unknown>).tool_delivery_errors;
  if (!Array.isArray(errors)) return [];
  return errors.flatMap((error) => {
    if (!error || typeof error !== "object" || Array.isArray(error)) return [];
    const message = (error as Record<string, unknown>).message;
    return typeof message === "string" && message.trim() !== ""
      ? [message]
      : [];
  });
}

function runDuration(run: AutomationRun, now = Date.now()): string {
  const start = Date.parse(run.triggered_at || run.created_at);
  if (!Number.isFinite(start)) return "—";
  const end = run.completed_at ? Date.parse(run.completed_at) : now;
  if (!Number.isFinite(end) || end < start) return "—";
  const seconds = Math.floor((end - start) / 1000);
  if (seconds < 60) return "< 1m";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
}

export function useRunHistoryClock(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

function triggerLabel(
  run: AutomationRun,
  triggers: AutomationTrigger[],
  t: ReturnType<typeof useT<"automations">>["t"],
): string {
  if (run.source === "manual") return t(($) => $.run_history.test_run);
  const trigger = run.trigger_id
    ? triggers.find((candidate) => candidate.id === run.trigger_id)
    : null;
  if (trigger?.label?.trim()) return trigger.label;
  return t(($) => $.run_source[run.source]);
}

function RunStatusBadge({ status }: { status: RunStatus }) {
  const { t } = useT("automations");
  return (
    <Badge
      variant={status === "failed" ? "destructive" : "secondary"}
      className={cn(
        "h-5 border-0 px-2 font-medium",
        status === "completed" &&
          "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
        (status === "running" || status === "issue_created") &&
          "bg-blue-500/10 text-blue-600 dark:text-blue-400",
        status === "skipped" && "text-muted-foreground",
      )}
    >
      {status === "running" ? (
        <Loader2 className="animate-spin" aria-hidden="true" />
      ) : null}
      {t(($) => $.run_status[status])}
    </Badge>
  );
}

function RunRow({
  run,
  agentId,
  triggers,
  now,
  automationTitle,
}: {
  automationTitle?: string;
  run: AutomationRun;
  agentId: string;
  triggers: AutomationTrigger[];
  now: number;
}) {
  const { t, i18n } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const status = normalizedRunStatus(run.status);
  const deliveryErrors = getToolDeliveryErrors(run.result);
  const [threadOpen, setThreadOpen] = useState(false);

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
          run.status === "running"
            ? "running"
            : run.status === "completed"
              ? "completed"
              : run.status === "failed"
                ? "failed"
                : "queued",
        priority: 0,
        dispatched_at: null,
        started_at: run.triggered_at || null,
        completed_at: run.completed_at || null,
        result: null,
        error: run.failure_reason || null,
        created_at: run.created_at,
      }
    : null;

  return (
    <>
      <TableRow data-testid="automation-run-row" className="h-14">
        <TableCell className="min-w-40 max-w-64 px-5">
          <span className="flex min-w-0 items-center gap-2">
            <Clock
              className="size-3.5 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <span className="truncate font-medium">
              {automationTitle ? (
                <AppLink href={wsPaths.automationDetail(run.automation_id)}>
                  {automationTitle}
                </AppLink>
              ) : (
                triggerLabel(run, triggers, t)
              )}
            </span>
          </span>
        </TableCell>
        <TableCell className="text-muted-foreground tabular-nums">
          {formatInTimeZone(
            run.triggered_at || run.created_at,
            undefined,
            i18n.language,
          )}
        </TableCell>
        <TableCell className="text-muted-foreground">—</TableCell>
        <TableCell>
          <div className="flex max-w-64 flex-col items-start gap-1">
            <RunStatusBadge status={status} />
            {run.failure_reason ? (
              <span
                title={run.failure_reason}
                className="line-clamp-2 whitespace-normal break-words text-caption text-destructive"
              >
                {run.failure_reason}
              </span>
            ) : null}
          </div>
        </TableCell>
        <TableCell className="text-muted-foreground tabular-nums">
          {runDuration(run, now)}
        </TableCell>
        <TableCell className="w-10 px-3 text-right">
          {syntheticTask ? (
            <AgentThreadButton
              task={syntheticTask}
              title={t(($) => $.run.view_conversation)}
              renderButton={false}
              open={threadOpen}
              onOpenChange={setThreadOpen}
            />
          ) : null}
          {syntheticTask || run.issue_id ? (
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    type="button"
                    size="icon-sm"
                    variant="ghost"
                    aria-label={t(($) => $.run_history.actions)}
                  />
                }
              >
                <MoreHorizontal aria-hidden="true" />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {syntheticTask ? (
                  <DropdownMenuItem onClick={() => setThreadOpen(true)}>
                    {t(($) => $.run.view_conversation)}
                  </DropdownMenuItem>
                ) : null}
                {run.issue_id ? (
                  <DropdownMenuItem
                    render={
                      <AppLink href={wsPaths.issueDetail(run.issue_id)} />
                    }
                  >
                    {t(($) => $.run_history.open_issue)}
                  </DropdownMenuItem>
                ) : null}
              </DropdownMenuContent>
            </DropdownMenu>
          ) : null}
        </TableCell>
      </TableRow>
      {deliveryErrors.length > 0 && (
        <TableRow className="hover:bg-transparent">
          <TableCell colSpan={6} className="px-5 pb-3 pt-0">
            <div
              role="alert"
              className="flex items-start gap-1.5 rounded-md bg-destructive/10 px-2.5 py-2 text-caption text-destructive"
            >
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <span>
                <span className="font-medium">
                  {t(($) => $.run.delivery_failed)}:
                </span>{" "}
                {deliveryErrors.join(" · ")}
              </span>
            </div>
          </TableCell>
        </TableRow>
      )}
    </>
  );
}

export function RunHistoryList({
  runs,
  agentId,
  triggers,
  toolbarStart,
  automations,
  onFiltersChange,
  beforeTable,
  loading = false,
}: {
  runs: AutomationRun[];
  agentId: string;
  triggers: AutomationTrigger[];
  onFiltersChange?: (search: string, statuses: string[]) => void;
  loading?: boolean;
  beforeTable?: ReactNode;
  toolbarStart?: ReactNode;
  automations?: Record<string, { title: string; executor_id: string }>;
}) {
  const { t } = useT("automations");
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [statuses, setStatuses] = useState<Set<RunStatus>>(new Set());
  useEffect(() => {
    onFiltersChange?.(search, [...statuses]);
  }, [search, statuses, onFiltersChange]);
  const hasActiveRun = runs.some(
    (run) =>
      !run.completed_at &&
      (normalizedRunStatus(run.status) === "running" ||
        normalizedRunStatus(run.status) === "issue_created"),
  );
  const now = useRunHistoryClock(hasActiveRun);
  const visibleRuns = useMemo(() => {
    if (onFiltersChange) return runs;
    const needle = search.trim().toLocaleLowerCase();
    return runs.filter((run) => {
      const status = normalizedRunStatus(run.status);
      if (statuses.size > 0 && !statuses.has(status)) return false;
      if (!needle) return true;
      const haystack = [
        automations?.[run.automation_id]?.title ??
          triggerLabel(run, triggers, t),
        t(($) => $.run_status[status]),
        run.failure_reason ?? "",
      ]
        .join(" ")
        .toLocaleLowerCase();
      return haystack.includes(needle);
    });
  }, [runs, search, statuses, t, triggers, automations, onFiltersChange]);

  const toggleStatus = (status: RunStatus) => {
    setStatuses((current) => {
      const next = new Set(current);
      if (next.has(status)) next.delete(status);
      else next.add(status);
      return next;
    });
  };

  return (
    <div className="space-y-3">
      <div className="flex min-h-8 flex-wrap items-center gap-1.5">
        <div className="mr-auto">{toolbarStart}</div>
        {searchOpen ? (
          <div className="relative w-full max-w-64">
            <Search
              className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
              aria-hidden="true"
            />
            <Input
              autoFocus
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t(($) => $.run_history.search_placeholder)}
              aria-label={t(($) => $.run_history.search_placeholder)}
              className="h-8 pl-8"
            />
          </div>
        ) : null}
        <Button
          type="button"
          size="icon-sm"
          variant="ghost"
          aria-label={t(($) => $.run_history.search)}
          aria-pressed={searchOpen}
          onClick={() => {
            setSearchOpen((open) => !open);
            if (searchOpen) setSearch("");
          }}
        >
          <Search aria-hidden="true" />
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                type="button"
                size="icon-sm"
                variant={statuses.size > 0 ? "secondary" : "ghost"}
                aria-label={t(($) => $.run_history.filter)}
              />
            }
          >
            <ListFilter aria-hidden="true" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-44">
            <DropdownMenuGroup>
              <DropdownMenuLabel>
                {t(($) => $.run_history.filter_status)}
              </DropdownMenuLabel>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            {RUN_STATUSES.map((status) => (
              <DropdownMenuCheckboxItem
                key={status}
                checked={statuses.has(status)}
                onCheckedChange={() => toggleStatus(status)}
                onSelect={(event) => event.preventDefault()}
              >
                {t(($) => $.run_status[status])}
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {beforeTable}
      <div className="overflow-hidden rounded-xl border bg-background">
        <Table>
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead className="px-5">
                {automations
                  ? t(($) => $.page.title)
                  : t(($) => $.run_history.trigger)}
              </TableHead>
              <TableHead>{t(($) => $.run_history.triggered)}</TableHead>
              <TableHead>{t(($) => $.run_history.tools)}</TableHead>
              <TableHead>{t(($) => $.run_history.status)}</TableHead>
              <TableHead>{t(($) => $.run_history.duration)}</TableHead>
              <TableHead className="w-10">
                <span className="sr-only">
                  {t(($) => $.run_history.actions)}
                </span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visibleRuns.map((run) => (
              <RunRow
                key={run.id}
                run={run}
                agentId={
                  automations?.[run.automation_id]?.executor_id ?? agentId
                }
                automationTitle={automations?.[run.automation_id]?.title}
                triggers={triggers}
                now={now}
              />
            ))}
            {loading ? (
              <TableRow>
                <TableCell colSpan={6} className="h-28 px-5">
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ) : visibleRuns.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell
                  colSpan={6}
                  className="h-28 text-center text-muted-foreground"
                >
                  {t(($) => $.run_history.no_matches)}
                </TableCell>
              </TableRow>
            ) : null}
          </TableBody>
        </Table>
      </div>
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
            err instanceof Error
              ? err.message
              : t(($) => $.dialog.toast_update_failed),
          );
        },
      },
    );
  };

  return (
    <AutomationInstructionsCard
      description={automation.description ?? ""}
      assignee={{ type: automation.executor_type, id: automation.executor_id }}
      canWrite={canWrite}
      busy={updateAutomation.isPending}
      onDescriptionChange={persistDescription}
      onAssigneeChange={(next) => {
        if (!canWrite) return;
        updateAutomation.mutate(
          {
            id: automation.id,
            executor_type: next.type,
            executor_id: next.id,
            model: "",
          },
          {
            onError: (error) =>
              toast.error(
                error instanceof Error
                  ? error.message
                  : t(($) => $.dialog.toast_update_failed),
              ),
          },
        );
      }}
    />
  );
}

export function AutomationDetailPage({
  automationId,
}: {
  automationId: string;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const router = useNavigation();
  const { getActorName } = useActorName();

  const { data, isLoading, isError, refetch } = useQuery(
    automationDetailOptions(wsId, automationId),
  );
  const {
    data: runs = [],
    isLoading: runsLoading,
    isError: runsError,
    refetch: refetchRuns,
  } = useQuery(automationRunsOptions(wsId, automationId));
  const updateAutomation = useUpdateAutomation();
  const deleteAutomation = useDeleteAutomation();
  const triggerAutomation = useTriggerAutomation();
  const createTrigger = useCreateAutomationTrigger();

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
        <p role="status">
          {isError
            ? t(($) => $.settings.load_failed)
            : t(($) => $.detail.not_found)}
        </p>
        {isError && (
          <Button size="sm" variant="outline" onClick={() => void refetch()}>
            {t(($) => $.page.retry)}
          </Button>
        )}
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
      toast.error(
        clientErrorMessage(e) || t(($) => $.detail.toast_trigger_failed),
      );
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
    updateAutomation.mutate(
      { id: automationId, status: checked ? "active" : "paused" },
      {
        onError: (error) =>
          toast.error(
            error instanceof Error
              ? error.message
              : t(($) => $.dialog.toast_update_failed),
          ),
      },
    );
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
        err instanceof Error
          ? err.message
          : t(($) => $.settings.toast_trigger_add_failed),
      );
    }
  };

  const hasWebhookTrigger = triggers.some((trig) => trig.kind === "webhook");
  return (
    <div className="flex h-full flex-col">
      <AutomationSettingsPage
        title={automation.title}
        status={automation.status}
        creatorName={getActorName(
          automation.created_by_type,
          automation.created_by_id,
        )}
        projectId={automation.project_id ?? null}
        canWrite={canWrite}
        busy={updateAutomation.isPending}
        onToggleStatus={handleToggleStatus}
        onProjectChange={(projectId) =>
          updateAutomation.mutate(
            { id: automationId, project_id: projectId },
            {
              onError: (error) =>
                toast.error(
                  error instanceof Error
                    ? error.message
                    : t(($) => $.dialog.toast_update_failed),
                ),
            },
          )
        }
        tab={detailTab}
        onTabChange={setDetailTab}
        actions={
          canWrite ? (
            <>
              <Button
                size="sm"
                variant="outline"
                onClick={() => setEditDialogOpen(true)}
                className="px-2 sm:px-2.5"
                aria-label={t(($) => $.detail.edit)}
              >
                <Pencil className="h-3.5 w-3.5 sm:mr-1" />
                <span className="hidden sm:inline">
                  {t(($) => $.detail.edit)}
                </span>
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={handleRunNow}
                disabled={
                  automation.status !== "active" || triggerAutomation.isPending
                }
                className="px-2 sm:px-2.5"
                aria-label={
                  triggerAutomation.isPending
                    ? t(($) => $.detail.running)
                    : t(($) => $.detail.run_now)
                }
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
        notice={
          automation.pause_reason === "agent_runtime_required" && (
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
          )
        }
        inlineRunTabs={!runsLoading && !runsError && runs.length > 0}
        renderRuns={(tabs) => (
          <>
            {runsLoading ? (
              <div className="space-y-1">
                {Array.from({ length: 3 }).map((_, i) => (
                  <Skeleton key={i} className="h-10 w-full" />
                ))}
              </div>
            ) : runsError ? (
              <div className="space-y-3 rounded-md border border-dashed p-4 text-center text-body text-muted-foreground">
                <p role="status">{t(($) => $.settings.runs_load_failed)}</p>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void refetchRuns()}
                >
                  {t(($) => $.page.retry)}
                </Button>
              </div>
            ) : runs.length === 0 ? (
              <div className="rounded-md border border-dashed p-4 text-center text-body text-muted-foreground">
                {t(($) => $.detail.no_runs)}
              </div>
            ) : (
              <RunHistoryList
                runs={runs}
                toolbarStart={tabs}
                agentId={automation.executor_id}
                triggers={triggers}
              />
            )}
          </>
        )}
      >
        <AutomationTriggersSection
          canWrite={canWrite}
          onPickSchedule={() => setTriggerDialogOpen(true)}
          onPickPreset={handlePickPreset}
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
        </AutomationTriggersSection>

        <InstructionsSection automation={automation} canWrite={canWrite} />

        <AutomationToolsSection
          automation={automation}
          assignee={{ type: automation.executor_type, id: automation.executor_id }}
          canWrite={canWrite}
        />

        <WebhookDeliveriesSection
          automationId={automationId}
          hasWebhookTrigger={hasWebhookTrigger}
        />
      </AutomationSettingsPage>

      {/* Mounted only while open, like the edit dialog: otherwise a rejected
          cron leaves scheduleValid=false behind for the next open. */}
      {triggerDialogOpen && (
        <TriggerScheduleDialog
          onOpenChange={setTriggerDialogOpen}
          automationId={automationId}
        />
      )}
      {editDialogOpen && (
        <AutomationEditDialog
          open={editDialogOpen}
          onOpenChange={setEditDialogOpen}
          automationId={automation.id}
          initial={{
            title: automation.title,
            description: automation.description ?? "",
            project_id: automation.project_id ?? null,
            executor_type: automation.executor_type,
            executor_id: automation.executor_id,
            execution_mode:
              automation.execution_mode as AutomationExecutionMode,
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
        onOpenChange={(v) => {
          if (!v && !deleting) setDeleteConfirmOpen(false);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.detail.delete_dialog.title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.detail.delete_dialog.description, {
                title: automation.title,
              })}
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
            <AlertDialogTitle>
              {t(($) => $.detail.run_blocked_dialog.title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.detail.run_blocked_dialog.description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogAction
              onClick={() => {
                setRunBlockedOpen(false);
                setDetailTab("settings");
              }}
            >
              {t(($) => $.detail.run_blocked_dialog.confirm)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
