"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronRight,
  Cloud,
  Monitor,
  Plus,
  Server,
} from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { agentTaskSnapshotOptions } from "@orvilo/core/agents";
import { runtimeProfileListOptions } from "@orvilo/core/runtimes";
import { runtimeListOptions, runtimeKeys } from "@orvilo/core/runtimes/queries";
import { useWSEvent } from "@orvilo/core/realtime";
import { agentListOptions } from "@orvilo/core/workspace/queries";
import { Button } from "@orvilo/ui/components/ui/button";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import {
  CollectionPageHeaderAction,
  CollectionPageState,
} from "../../layout/collection-page";
import { ShellHeaderActions } from "../../layout/shell-header";
import { AppLink } from "../../navigation";
import { ConnectRemoteDialog } from "./connect-remote-dialog";
import { CloudRuntimeDialog } from "./cloud-runtime-dialog";
import { buildWorkloadIndex, RuntimeList } from "./runtime-list";
import { pendingRuntimeFromProfile } from "./pending-runtime";
import { buildRuntimeMachines, type RuntimeMachine } from "./runtime-machines";
import { HealthDot, useHealthLabel } from "./shared";
import { useT } from "../../i18n";

export interface RuntimesPageProps {
  /** Desktop-only daemon id used to identify this device. */
  localDaemonId?: string | null;
  /** Desktop-only friendly device name for the local daemon. */
  localMachineName?: string | null;
  /** Keep the local device visible even before its first runtime registers. */
  hasLocalMachine?: boolean;
  /** The bundled daemon is starting but has not registered yet. */
  bootstrapping?: boolean;
  /** Web SaaS-only Cloud Runtime entrypoint. */
  cloudRuntimeEnabled?: boolean;
}

function useNowTick(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

export function RuntimesPage({
  localDaemonId,
  localMachineName,
  hasLocalMachine,
  bootstrapping,
  cloudRuntimeEnabled = false,
}: RuntimesPageProps = {}) {
  const isAuthLoading = useAuthStore((state) => state.isLoading);
  const currentUserId = useAuthStore((state) => state.user?.id);
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const [showConnectDialog, setShowConnectDialog] = useState(false);
  const [showCloudRuntimeDialog, setShowCloudRuntimeDialog] = useState(false);

  const { data: runtimes = [], isLoading: runtimesLoading } = useQuery(
    runtimeListOptions(wsId),
  );
  const { data: runtimeProfiles = [], isLoading: profilesLoading } = useQuery(
    runtimeProfileListOptions(wsId),
  );
  const { data: agents = [] } = useQuery(
    agentListOptions(wsId),
  );
  const { data: snapshot = [] } = useQuery(agentTaskSnapshotOptions(wsId));
  const handleDaemonEvent = useCallback(() => {
    qc.invalidateQueries({ queryKey: runtimeKeys.all(wsId) });
  }, [qc, wsId]);
  useWSEvent("daemon:register", handleDaemonEvent);

  const workloadIndex = useMemo(
    () => buildWorkloadIndex(agents, snapshot),
    [agents, snapshot],
  );
  const now = useNowTick();
  const machines = useMemo(
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
  const orphanProfileRuntimes = useMemo(() => {
    if (machines.some((machine) => machine.mode === "local")) return [];
    return runtimeProfiles.map((profile) => {
      const createdAt = Date.parse(profile.created_at);
      return pendingRuntimeFromProfile({
        profile,
        createdAt: Number.isFinite(createdAt) ? createdAt : 0,
        fallbackMachineName: "Unassigned",
      });
    });
  }, [machines, runtimeProfiles]);

  if (isAuthLoading || runtimesLoading || profilesLoading) {
    return <RuntimesPageSkeleton />;
  }

  const showEmpty =
    machines.length === 0 &&
    orphanProfileRuntimes.length === 0 &&
    !bootstrapping &&
    hasLocalMachine !== true;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <RuntimesHeaderActions
        onConnectRemote={() => setShowConnectDialog(true)}
        cloudRuntimeEnabled={cloudRuntimeEnabled}
        onOpenCloudRuntime={() => setShowCloudRuntimeDialog(true)}
      />

      {showEmpty ? (
        <div className="flex flex-1 items-center justify-center p-6">
          <EmptyState onConnectRemote={() => setShowConnectDialog(true)} />
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto flex w-full max-w-3xl flex-col p-4 sm:p-6">
            {(machines.length > 0 || bootstrapping) && (
              <MachineList
                machines={machines}
                bootstrapping={bootstrapping}
              />
            )}
            {orphanProfileRuntimes.length > 0 && (
              <OrphanRuntimeProfiles
                runtimes={orphanProfileRuntimes}
                now={now}
                hasMachines={machines.length > 0}
              />
            )}
          </div>
        </div>
      )}

      {showConnectDialog && (
        <ConnectRemoteDialog onClose={() => setShowConnectDialog(false)} />
      )}
      {cloudRuntimeEnabled && showCloudRuntimeDialog && (
        <CloudRuntimeDialog onClose={() => setShowCloudRuntimeDialog(false)} />
      )}
    </div>
  );
}

function OrphanRuntimeProfiles({
  runtimes,
  now,
  hasMachines,
}: {
  runtimes: ReturnType<typeof pendingRuntimeFromProfile>[];
  now: number;
  hasMachines: boolean;
}) {
  const { t } = useT("runtimes");
  return (
    <section className={hasMachines ? "mt-6" : undefined}>
      <div className="mb-3">
        <h2 className="text-body font-semibold">
          {t(($) => $.profiles.unassigned_title)}
        </h2>
        <p className="mt-1 text-caption text-muted-foreground">
          {t(($) => $.profiles.unassigned_description)}
        </p>
      </div>
      <RuntimeList runtimes={runtimes} now={now} />
    </section>
  );
}

function RuntimesHeaderActions({
  onConnectRemote,
  cloudRuntimeEnabled,
  onOpenCloudRuntime,
}: {
  onConnectRemote: () => void;
  cloudRuntimeEnabled: boolean;
  onOpenCloudRuntime: () => void;
}) {
  const { t } = useT("runtimes");
  return (
    <ShellHeaderActions>
      {cloudRuntimeEnabled && (
        <CollectionPageHeaderAction
          icon={Cloud}
          label={t(($) => $.cloud_runtime.action)}
          onClick={onOpenCloudRuntime}
        />
      )}
      <CollectionPageHeaderAction
        icon={Plus}
        label={t(($) => $.page.connect_remote)}
        onClick={onConnectRemote}
      />
    </ShellHeaderActions>
  );
}

function MachineList({
  machines,
  bootstrapping,
}: {
  machines: RuntimeMachine[];
  bootstrapping?: boolean;
}) {
  const { t } = useT("runtimes");
  if (machines.length === 0) {
    return (
      <CollectionPageState
        icon={Server}
        title={
          bootstrapping
            ? t(($) => $.page.bootstrapping.title)
            : t(($) => $.page.empty.title)
        }
        description={
          bootstrapping
            ? t(($) => $.page.bootstrapping.hint)
            : t(($) => $.page.empty.hint)
        }
      />
    );
  }

  return (
    <div className="overflow-hidden rounded-xl border bg-card px-4">
      <div className="divide-y">
        {machines.map((machine) => (
          <MachineRow key={machine.id} machine={machine} />
        ))}
      </div>
    </div>
  );
}

function MachineRow({ machine }: { machine: RuntimeMachine }) {
  const healthLabel = useHealthLabel();
  const paths = useWorkspacePaths();
  const Icon = machine.section === "cloud" ? Cloud : Monitor;
  const health = machine.health === "online" ? "online" : "offline";

  return (
    <AppLink
      href={paths.runtimeDetail(machine.id)}
      className="group flex min-w-0 items-center gap-4 py-5 transition-colors hover:bg-accent/40 focus-visible:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    >
      <span className="flex size-12 shrink-0 items-center justify-center rounded-lg bg-muted/50">
        <Icon aria-hidden="true" className="size-5 text-muted-foreground" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-title-sm font-medium">{machine.title}</span>
        <span className="mt-1 flex items-center gap-2 text-body text-muted-foreground">
          <HealthDot health={health} />
          {healthLabel(health)}
        </span>
      </span>
      <ChevronRight
        aria-hidden="true"
        className="size-4 shrink-0 text-faint-foreground transition-transform group-hover:translate-x-0.5 group-hover:text-muted-foreground"
      />
    </AppLink>
  );
}

function EmptyState({ onConnectRemote }: { onConnectRemote: () => void }) {
  const { t } = useT("runtimes");
  return (
    <CollectionPageState
      icon={Server}
      title={t(($) => $.page.empty.title)}
      description={t(($) => $.page.empty.hint)}
      actions={
        <Button type="button" size="sm" onClick={onConnectRemote}>
          <Plus aria-hidden="true" className="size-3" />
          {t(($) => $.page.connect_remote)}
        </Button>
      }
    />
  );
}

function RuntimesPageSkeleton() {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="mx-auto w-full max-w-3xl p-6">
        <div className="overflow-hidden rounded-lg border">
          {Array.from({ length: 5 }).map((_, index) => (
            <div key={index} className="flex h-[88px] items-center gap-4 border-b px-4 last:border-b-0">
              <Skeleton className="size-12 rounded-lg" />
              <div className="flex-1">
                <Skeleton className="h-4 w-44" />
                <Skeleton className="mt-2 h-3 w-28" />
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export default RuntimesPage;
