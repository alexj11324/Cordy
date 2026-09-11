"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertCircle, Cloud, Monitor, Pencil, Plus, Server, Trash2 } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { agentTaskSnapshotOptions } from "@orvilo/core/agents";
import { runtimeProfileListOptions } from "@orvilo/core/runtimes";
import { runtimeKeys, runtimeListOptions } from "@orvilo/core/runtimes/queries";
import { useWSEvent } from "@orvilo/core/realtime";
import {
  agentListOptions,
  memberListOptions,
} from "@orvilo/core/workspace/queries";
import { Button } from "@orvilo/ui/components/ui/button";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@orvilo/ui/components/ui/tooltip";
import { AppLink, useNavigation } from "../../navigation";
import { BreadcrumbHeader } from "../../layout/breadcrumb-header";
import { buildWorkloadIndex, RuntimeList } from "./runtime-list";
import {
  buildRuntimeMachines,
  sharedCustomName,
} from "./runtime-machines";
import { RenameMachineDialog } from "./rename-machine-dialog";
import {
  canDeleteRuntimeMachine,
  DeleteMachineDialog,
} from "./delete-machine-dialog";
import { RuntimeProfilesDialog } from "./runtime-profiles-dialog";
import { pendingRuntimesForProfiles } from "./pending-runtime";
import { HealthDot, HealthIcon, useHealthLabel } from "./shared";
import { useT } from "../../i18n";

export interface RuntimeDetailPageProps {
  /** A machine id, or a legacy runtime id that locates its machine. */
  runtimeId: string;
  localDaemonId?: string | null;
  localMachineName?: string | null;
  localMachineActions?: React.ReactNode;
  hasLocalMachine?: boolean;
  bootstrapping?: boolean;
  onStopLocalDaemon?: () => Promise<void>;
}

function useNowTick(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

function decodeRouteParam(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function findMachine(
  machines: ReturnType<typeof buildRuntimeMachines>,
  locator: string,
) {
  return (
    machines.find(
      (candidate) =>
        candidate.id === locator ||
        candidate.runtimes.some((runtime) => runtime.id === locator),
    ) ??
    (locator === "local:placeholder"
      ? machines.find((candidate) => candidate.isCurrent) ?? null
      : null)
  );
}

/**
 * Machine-level detail route. New links use the daemon-level machine id;
 * legacy links that still carry a runtime id remain valid and are expanded
 * to their containing machine.
 */
export function RuntimeDetailPage({
  runtimeId,
  localDaemonId,
  localMachineName,
  localMachineActions,
  hasLocalMachine,
  bootstrapping,
  onStopLocalDaemon,
}: RuntimeDetailPageProps) {
  const { t } = useT("runtimes");
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const navigation = useNavigation();
  const qc = useQueryClient();
  const healthLabel = useHealthLabel();
  const currentUserId = useAuthStore((state) => state.user?.id);
  const { data: runtimes = [], isLoading } = useQuery(runtimeListOptions(wsId));
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: tasks = [], isLoading: tasksLoading } = useQuery(
    agentTaskSnapshotOptions(wsId),
  );
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: runtimeProfiles = [] } = useQuery(
    runtimeProfileListOptions(wsId),
  );
  const now = useNowTick();
  const machineLocator = decodeRouteParam(runtimeId);
  const [renameOpen, setRenameOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [createProfileOpen, setCreateProfileOpen] = useState(false);
  const workloadIndex = useMemo(
    () => buildWorkloadIndex(agents, tasks),
    [agents, tasks],
  );
  const baseMachines = useMemo(
    () =>
      buildRuntimeMachines(runtimes, {
        now,
        localDaemonId,
        localMachineName,
        currentUserId,
        workloadByRuntimeId: workloadIndex,
        ensureLocalMachine: hasLocalMachine,
      }),
    [
      runtimes,
      now,
      localDaemonId,
      localMachineName,
      currentUserId,
      workloadIndex,
      hasLocalMachine,
    ],
  );
  const baseMachine = findMachine(baseMachines, machineLocator);
  const profileRows = useMemo(
    () =>
      runtimeProfiles.map((profile) => {
        const createdAt = Date.parse(profile.created_at);
        return {
          profile,
          createdAt: Number.isFinite(createdAt) ? createdAt : 0,
        };
      }),
    [runtimeProfiles],
  );
  const machineRuntimes = useMemo(() => {
    if (!baseMachine) return [];
    if (baseMachine.mode !== "local") return baseMachine.runtimes;
    return pendingRuntimesForProfiles({
      pendingProfiles: profileRows,
      runtimes: baseMachine.runtimes,
      localDaemonId: baseMachine.daemonId,
      localMachineName: baseMachine.title,
      fallbackMachineName: baseMachine.title,
    });
  }, [baseMachine, profileRows]);
  const machine = baseMachine;
  const handleDaemonEvent = useCallback(() => {
    qc.invalidateQueries({ queryKey: runtimeKeys.all(wsId) });
  }, [qc, wsId]);
  useWSEvent("daemon:register", handleDaemonEvent);

  const currentMember = currentUserId
    ? members.find((member) => member.user_id === currentUserId)
    : null;
  const isAdmin =
    currentMember?.role === "owner" || currentMember?.role === "admin";
  const canAddRuntime =
    isAdmin && machine?.mode === "local" && !!machine.daemonId;
  const renameTarget = useMemo(() => {
    if (!machine || machine.runtimes.length === 0) return null;
    const editable = isAdmin
      ? machine.runtimes[0]
      : machine.runtimes.find((runtime) => runtime.owner_id === currentUserId);
    if (!editable) return null;
    return {
      runtimeId: editable.id,
      currentName: sharedCustomName(machine.runtimes) ?? "",
    };
  }, [machine, isAdmin, currentUserId]);
  const canDeleteMachine = canDeleteRuntimeMachine(machine, {
    isAdmin,
    currentUserId,
  });

  if (isLoading) return <MachineDetailSkeleton />;

  if (!machine) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 py-16 text-center">
          <AlertCircle className="h-8 w-8 text-destructive" />
          <div>
            <p className="text-body font-medium">
              {t(($) => $.machine.not_found_title)}
            </p>
            <p className="mt-1 text-caption text-muted-foreground">
              {t(($) => $.machine.not_found_hint)}
            </p>
          </div>
          <Button
            size="sm"
            render={<AppLink href={paths.runtimes()} />}
            nativeButton={false}
          >
            {t(($) => $.detail.all_runtimes)}
          </Button>
        </div>
      </div>
    );
  }

  const Icon = machine.section === "cloud" ? Cloud : Monitor;
  const busyCount = machine.runningCount + machine.queuedCount;
  const isOnline = machine.health === "online";
  const workloadLabel = tasksLoading
    ? null
    : busyCount > 0
      ? t(($) => $.machine.metrics.workload_value_busy)
      : t(($) => $.machine.metrics.workload_value_idle);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <BreadcrumbHeader
        segments={[{ href: paths.devices(), label: t(($) => $.page.title) }]}
        leaf={<span className="min-w-0 truncate">{machine.title}</span>}
      />
      <header className="shrink-0 border-b bg-background px-4 pb-5 pt-3 sm:px-6">
        <div className="mx-auto max-w-4xl">
          <div className="mt-4 flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
            <div className="flex min-w-0 items-start gap-4">
              <div className="relative flex h-12 w-12 shrink-0 items-center justify-center rounded-xl border bg-card sm:h-14 sm:w-14">
                <Icon
                  aria-hidden="true"
                  className="h-5 w-5 text-muted-foreground sm:h-6 sm:w-6"
                />
                {isOnline && (
                  <HealthDot
                    health="online"
                    className="absolute bottom-1 right-1 ring-2 ring-card"
                  />
                )}
              </div>
              <div className="min-w-0 pt-0.5">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                  <h1 className="min-w-0 text-balance text-title-lg font-semibold tracking-tight sm:text-display-sm">
                    {machine.title}
                  </h1>
                  {isOnline ? (
                    workloadLabel && (
                      <span
                        className={
                          busyCount > 0
                            ? "text-caption text-warning"
                            : "text-caption text-muted-foreground"
                        }
                      >
                        {workloadLabel}
                      </span>
                    )
                  ) : (
                    <span className="inline-flex items-center gap-1.5 text-caption">
                      <HealthIcon health={machine.health} />
                      {healthLabel(machine.health)}
                    </span>
                  )}
                  {machine.isCurrent && (
                    <span className="rounded bg-foreground px-1.5 py-0.5 text-micro font-medium text-background">
                      {t(($) => $.machine.this_machine)}
                    </span>
                  )}
                </div>
              </div>
            </div>

            <div className="flex shrink-0 items-center gap-2 self-end lg:self-start">
              {renameTarget && (
                <Tooltip>
                  <TooltipTrigger
                    render={
                      <Button
                        type="button"
                        size="icon"
                        variant="outline"
                        aria-label={t(($) => $.machine.rename)}
                        onClick={() => setRenameOpen(true)}
                      >
                        <Pencil aria-hidden="true" className="h-3.5 w-3.5" />
                      </Button>
                    }
                  />
                  <TooltipContent>{t(($) => $.machine.rename)}</TooltipContent>
                </Tooltip>
              )}
              {canDeleteMachine && (
                <Tooltip>
                  <TooltipTrigger
                    render={
                      <Button
                        type="button"
                        size="icon"
                        variant="outline"
                        aria-label={t(($) => $.machine.delete)}
                        onClick={() => setDeleteOpen(true)}
                      >
                        <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
                      </Button>
                    }
                  />
                  <TooltipContent>{t(($) => $.machine.delete)}</TooltipContent>
                </Tooltip>
              )}
              {machine.isCurrent && localMachineActions}
            </div>
          </div>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto bg-background">
        <div className="mx-auto w-full max-w-4xl p-4 sm:p-6">
          {machineRuntimes.length > 0 ? (
            <RuntimeList
              runtimes={machineRuntimes}
              now={now}
              machineTitle={machine.title}
              headerAction={
                canAddRuntime ? (
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => setCreateProfileOpen(true)}
                  >
                    <Plus aria-hidden="true" className="h-3.5 w-3.5" />
                    {t(($) => $.profiles.add_custom)}
                  </Button>
                ) : undefined
              }
            />
          ) : (
            <>
              <div className="mb-4 flex items-center justify-between">
                <h2 className="text-body font-semibold">
                  {t(($) => $.machine.metrics.runtimes)}
                </h2>
                {canAddRuntime && (
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => setCreateProfileOpen(true)}
                  >
                    <Plus aria-hidden="true" className="h-3.5 w-3.5" />
                    {t(($) => $.profiles.add_custom)}
                  </Button>
                )}
              </div>
              <div className="flex min-h-48 flex-col items-center justify-center rounded-lg border border-dashed px-6 text-center">
                <Server
                  aria-hidden="true"
                  className="h-7 w-7 text-faint-foreground"
                />
                <p className="mt-3 text-body font-medium">
                  {bootstrapping
                    ? t(($) => $.page.bootstrapping.title)
                    : t(($) => $.machine.no_runtimes_title)}
                </p>
                <p className="mt-1 max-w-sm text-caption text-muted-foreground">
                  {bootstrapping
                    ? t(($) => $.page.bootstrapping.hint)
                    : t(($) => $.machine.no_runtimes_hint)}
                </p>
              </div>
            </>
          )}
        </div>
      </div>

      {renameTarget && (
        <RenameMachineDialog
          open={renameOpen}
          onOpenChange={setRenameOpen}
          wsId={wsId}
          runtimeId={renameTarget.runtimeId}
          currentName={renameTarget.currentName}
        />
      )}
      {canDeleteMachine && machine && (
        <DeleteMachineDialog
          open={deleteOpen}
          onOpenChange={setDeleteOpen}
          machine={machine}
          wsId={wsId}
          onStopLocalDaemon={
            machine.isCurrent ? onStopLocalDaemon : undefined
          }
          onDeleted={() => navigation.push(paths.devices())}
        />
      )}
      {canAddRuntime && createProfileOpen && (
        <RuntimeProfilesDialog
          wsId={wsId}
          intent="create"
          machineName={machine.title}
          onClose={() => setCreateProfileOpen(false)}
        />
      )}
    </div>
  );
}

function MachineDetailSkeleton() {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-b px-6 pb-5 pt-3">
        <Skeleton className="h-3 w-36" />
        <div className="mt-4 flex items-start gap-4">
          <Skeleton className="h-14 w-14 rounded-xl" />
          <div className="flex-1">
            <Skeleton className="h-7 w-64" />
            <Skeleton className="mt-2 h-4 w-40" />
            <Skeleton className="mt-3 h-3 w-72" />
          </div>
        </div>
      </div>
      <div className="mx-auto w-full max-w-4xl p-6">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="mt-2 h-3 w-72" />
        <div className="mt-4 overflow-hidden rounded-lg border">
          {Array.from({ length: 4 }).map((_, index) => (
            <Skeleton key={index} className="h-14 w-full rounded-none border-b last:border-b-0" />
          ))}
        </div>
      </div>
    </div>
  );
}
