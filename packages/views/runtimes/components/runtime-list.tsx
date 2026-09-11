"use client";

import { useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  AlertTriangle,
  Globe,
  Loader2,
  MoreHorizontal,
  Pencil,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import { useQuery } from "@tanstack/react-query";
import { useTable } from "@tanstack/react-table";
import type { ColumnDef } from "@tanstack/react-table";
import type {
  Agent,
  AgentRuntime,
  AgentTask,
  RuntimeProfile,
} from "@orvilo/core/types";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import {
  agentListOptions,
  memberListOptions,
} from "@orvilo/core/workspace/queries";
import { agentTaskSnapshotOptions } from "@orvilo/core/agents";
import {
  deriveRuntimeHealth,
  runtimeProfileListOptions,
} from "@orvilo/core/runtimes";
import {
  DataGrid,
  dataGridFeatures,
  type DataGridFeatures,
} from "@orvilo/ui/components/reui/data-grid/data-grid";
import { DataGridTable } from "@orvilo/ui/components/reui/data-grid/data-grid-table";
import {
  Frame,
  FrameHeader,
  FramePanel,
} from "@orvilo/ui/components/reui/frame";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@orvilo/ui/components/ui/tooltip";
import { ProviderLogo } from "./provider-logo";
import { HealthIcon, useHealthLabel } from "./shared";
import { DeleteRuntimeDialog } from "./delete-runtime-dialog";
import { DeleteRuntimeProfileDialog } from "./delete-runtime-profile-dialog";
import { RuntimeProfilesDialog } from "./runtime-profiles-dialog";
import { runtimeRowLabel } from "./runtime-machines";
import {
  customRuntimeRegistrationFailure,
  isDisabledCustomRuntime,
  isPendingCustomRuntime,
  isPendingCustomRuntimeWarning,
  pendingRuntimeCommandName,
} from "./pending-runtime";
import { useT, useTimeAgo } from "../../i18n";

// The machine detail's Harness inventory uses the ReUI settings-14 table
// composition (Frame + DataGridTable), while keeping
// this view's existing query and action behavior. The source block is the
// settings-14 preview at https://reui.io/preview/base/settings-14?ref=mcp;
// only its table shell is reused here, without its demo API keys or controls.

interface RuntimeWorkload {
  agentIds: string[];
  runningCount: number;
  queuedCount: number;
}

const EMPTY_WORKLOAD: RuntimeWorkload = {
  agentIds: [],
  runningCount: 0,
  queuedCount: 0,
};

export interface RuntimeRow {
  runtime: AgentRuntime;
  profile: RuntimeProfile | null;
  workload: RuntimeWorkload;
  canDelete: boolean;
}

// Per-runtime workload snapshot — agent IDs serving this runtime (drives
// the avatar stack; .length doubles as the agent count) plus task counts
// split by status. Built once per render off the workspace-wide
// agents / agent-task-snapshot caches; filtered locally — no extra requests.
export function buildWorkloadIndex(
  agents: Agent[],
  tasks: AgentTask[],
): Map<string, RuntimeWorkload> {
  const result = new Map<string, RuntimeWorkload>();
  const agentToRuntime = new Map<string, string>();

  for (const a of agents) {
    if (!a.runtime_id || a.archived_at) continue;
    agentToRuntime.set(a.id, a.runtime_id);
    const entry =
      result.get(a.runtime_id) ?? {
        agentIds: [],
        runningCount: 0,
        queuedCount: 0,
      };
    entry.agentIds.push(a.id);
    result.set(a.runtime_id, entry);
  }
  for (const t of tasks) {
    const rid = agentToRuntime.get(t.agent_id);
    if (!rid) continue;
    const entry = result.get(rid);
    if (!entry) continue;
    if (t.status === "running") entry.runningCount += 1;
    else if (t.status === "queued" || t.status === "dispatched")
      entry.queuedCount += 1;
  }
  return result;
}

// ---------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------

function RuntimeNameCell({
  runtime,
  machineTitle,
}: {
  runtime: AgentRuntime;

  /**
   * The containing machine's title. Lets a per-runtime alias surface here
   * while a machine-level rename (shared by every runtime on the daemon)
   * collapses to the provider base so it isn't repeated on every row
   * (MUL-5248). Omitted when the row has no machine context (orphan custom
   * runtime profiles), where any alias is shown verbatim.
   */
  machineTitle?: string;
}) {
  const label = runtimeRowLabel(runtime, machineTitle ?? "");
  return (
    <div className="flex min-w-0 items-center gap-2">
      <div className="flex h-8 w-8 shrink-0 items-center justify-center">
        <ProviderLogo provider={runtime.provider} className="h-5 w-5" />
      </div>
      <div className="flex min-w-0 flex-1 items-center gap-1.5">
        <span className="block min-w-0 shrink truncate text-body font-medium">
          {label}
        </span>
        {runtime.profile_id && <RuntimeKindBadge />}
        <PendingRuntimeBadge runtime={runtime} />
        <VisibilityBadge runtime={runtime} />
      </div>
    </div>
  );
}

// Custom runtimes keep a small badge so they remain distinguishable from the
// built-in rows without repeating a built-in badge on every row.
function RuntimeKindBadge() {
  const { t } = useT("runtimes");
  return (
    <span
      className="inline-flex shrink-0 items-center rounded bg-info/10 px-1 text-micro font-medium text-info"
    >
      {t(($) => $.list.badge_custom)}
    </span>
  );
}

function PendingRuntimeBadge({ runtime }: { runtime: AgentRuntime }) {
  const { t } = useT("runtimes");
  if (!isPendingCustomRuntime(runtime)) return null;
  if (isDisabledCustomRuntime(runtime)) {
    return (
      <span className="inline-flex shrink-0 items-center rounded bg-muted px-1 text-micro font-medium text-muted-foreground">
        {t(($) => $.list.badge_disabled)}
      </span>
    );
  }
  return (
    <span className="inline-flex shrink-0 items-center rounded bg-warning/10 px-1 text-micro font-medium text-warning">
      {t(($) => $.list.badge_registering)}
    </span>
  );
}

// Only public is worth a badge — private is the default and rendering a
// `🔒 Private` chip on every row turns the whole column into noise.
function VisibilityBadge({ runtime }: { runtime: AgentRuntime }) {
  const { t } = useT("runtimes");
  if (runtime.visibility !== "public") return null;
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span className="inline-flex shrink-0 items-center gap-0.5 rounded bg-info/10 px-1 text-micro font-medium text-info">
            <Globe className="h-2.5 w-2.5" />
            {t(($) => $.detail.visibility_label.public)}
          </span>
        }
      />
      <TooltipContent>
        {t(($) => $.detail.visibility_hint.public)}
      </TooltipContent>
    </Tooltip>
  );
}

// Health with the load folded in as a "· N tasks" suffix — verbatim the
// same form as the agents list's status cell, so the two surfaces speak
// one language. The suffix is a unit-bearing count (running + queued);
// offline-ish rows skip it (health already says it all), idle rows skip
// it (idle is the unremarkable default). If "queued but nothing running"
// ever becomes a signal worth surfacing, it belongs to the HEALTH layer
// (a new deriveRuntimeHealth state), not to vocabulary hints here.
function HealthCell({
  runtime,
  workload,
  now,
}: {
  runtime: AgentRuntime;
  workload: RuntimeWorkload;
  now: number;
}) {
  const { t } = useT("runtimes");
  const { t: tAgents } = useT("agents");
  const labelOf = useHealthLabel();
  const timeAgo = useTimeAgo();
  if (isDisabledCustomRuntime(runtime)) {
    return (
      <div>
        <span className="text-caption text-muted-foreground">
          {t(($) => $.list.pending_health_disabled)}
        </span>
      </div>
    );
  }
  const registrationFailure = customRuntimeRegistrationFailure(runtime);
  if (registrationFailure) {
    return (
      <div className="flex min-w-0 items-center gap-1.5">
        <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-destructive" />
        <span
          className="block min-w-0 truncate text-caption text-destructive"
          title={registrationFailure}
        >
          {t(($) => $.list.pending_health_error)}
        </span>
      </div>
    );
  }
  if (isPendingCustomRuntime(runtime)) {
    const warning = isPendingCustomRuntimeWarning(runtime, now);
    return (
      <div className="flex min-w-0 items-center gap-1.5">
        {warning ? (
          <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-warning" />
        ) : (
          <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-info" />
        )}
        <span className="block min-w-0 truncate text-caption">
          {warning
            ? t(($) => $.list.pending_health_warning)
            : t(($) => $.list.pending_health)}
        </span>
      </div>
    );
  }

  const health = deriveRuntimeHealth(runtime, now);
  const offline = health === "offline" || health === "long_offline";
  const lastSeen = runtime.last_seen_at ? timeAgo(runtime.last_seen_at) : null;
  const active = workload.runningCount + workload.queuedCount;

  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <HealthIcon health={health} />
      <span className="block min-w-0 truncate text-caption">
        {labelOf(health)}
        {health !== "online" && lastSeen && (
          <span className="text-muted-foreground"> · {lastSeen}</span>
        )}
        {!offline && active > 0 && (
          <span className="text-muted-foreground">
            {" · "}
            {tAgents(($) => $.row.task_count, { count: active })}
          </span>
        )}
      </span>
    </div>
  );
}

export function CliCell({ runtime }: { runtime: AgentRuntime }) {
  const { t } = useT("runtimes");
  const failure = customRuntimeRegistrationFailure(runtime);
  if (failure) {
    const command = pendingRuntimeCommandName(runtime);
    return (
      <div className="flex min-w-0 flex-col text-caption">
        {command && (
          <span
            className="truncate font-mono text-muted-foreground"
            title={command}
          >
            {command}
          </span>
        )}
        <span className="truncate text-destructive" title={failure}>
          {failure}
        </span>
      </div>
    );
  }
  if (isPendingCustomRuntime(runtime)) {
    const command = pendingRuntimeCommandName(runtime);
    if (!command) {
      return (
        <span className="text-caption text-muted-foreground">
          {t(($) => $.list.pending_cli_unknown)}
        </span>
      );
    }
    return (
      <div className="flex min-w-0 items-center text-caption">
        <span
          className="truncate font-mono text-muted-foreground"
          title={command}
        >
          {command}
        </span>
      </div>
    );
  }

  if (runtime.runtime_mode === "cloud") {
    return <span className="text-caption text-faint-foreground">—</span>;
  }
  const meta = runtime.metadata as Record<string, unknown> | null;
  // `version` is the agent's own underlying CLI tool version — distinct per
  // provider (e.g. "2.1.5 (Claude Code)", "codex-cli 0.118.0", "0.42.0").
  // The separate `cli_version` is the shared orvilo daemon CLI, identical
  // for every runtime on one machine; surfacing it here made all agents
  // show the same number (#3838). The daemon CLI version and its update
  // prompt belong to the machine — they live in the machine header, not on a
  // per-agent row.
  const version =
    meta && typeof meta.version === "string" ? meta.version : null;

  if (!version) {
    return <span className="text-caption text-faint-foreground">—</span>;
  }

  return (
    <div className="flex min-w-0 items-center text-caption">
      <span className="truncate font-mono text-muted-foreground">
        {version}
      </span>
    </div>
  );
}

export function RuntimeRowMenu({
  runtime,
  profile,
  wsId,
  canDelete,
}: {
  runtime: AgentRuntime;
  profile: RuntimeProfile | null;
  wsId: string;
  canDelete: boolean;
}) {
  const { t } = useT("runtimes");
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const isCustomRuntime = !!runtime.profile_id;
  // Delete is the row's only management action; if the row can't run it, drop
  // the kebab entirely so the column doesn't render a near-empty popover. We
  // used to also hide it for self-healing runtimes (live local daemon
  // re-registers within seconds), but MUL-3352 surfaced that owners read
  // a missing kebab as "I lost my permission" rather than "the daemon
  // would undo this". The dialog now carries the self-heal warning and
  // the user gets to decide.

  if (!canDelete) {
    return <span aria-hidden />;
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <button
              type="button"
              aria-label={t(($) => $.list.row_actions_aria)}
              className="flex size-7 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-accent-foreground focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring group-hover/row:opacity-100 data-popup-open:bg-accent data-popup-open:opacity-100 data-popup-open:text-accent-foreground"
            >
              <MoreHorizontal className="size-4" />
            </button>
          }
        />
        <DropdownMenuContent align="end" className="w-40">
          {isCustomRuntime && profile && (
            <DropdownMenuItem onClick={() => setEditOpen(true)}>
              <Pencil aria-hidden="true" className="h-3.5 w-3.5" />
              {t(($) => $.list.edit_action)}
            </DropdownMenuItem>
          )}
          <DropdownMenuItem
            variant="destructive"
            onClick={() => setDeleteOpen(true)}
            title={t(($) => $.list.delete_permission_hint)}
          >
            <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
            {isCustomRuntime
              ? t(($) => $.list.delete_profile_action)
              : t(($) => $.list.delete_action)}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {isCustomRuntime && profile && editOpen && (
        <RuntimeProfilesDialog
          wsId={wsId}
          intent="edit"
          initialProfile={profile}
          onClose={() => setEditOpen(false)}
        />
      )}
      {isCustomRuntime && profile ? (
        <DeleteRuntimeProfileDialog
          open={deleteOpen}
          onOpenChange={setDeleteOpen}
          profile={profile}
          wsId={wsId}
          onDeleted={() => setDeleteOpen(false)}
        />
      ) : (
        <DeleteRuntimeDialog
          open={deleteOpen}
          onOpenChange={setDeleteOpen}
          runtime={runtime}
          wsId={wsId}
          onDeleted={() => {
            setDeleteOpen(false);
            toast.success(t(($) => $.detail.toast_deleted));
          }}
        />
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

export function RuntimeList({
  runtimes,
  now,
  machineTitle,
  headerAction,
}: {
  runtimes: AgentRuntime[];
  now: number;
  /**
   * The containing machine's title, when this list renders the runtimes of a
   * single machine. Used so a machine-level alias doesn't repeat on every row
   * while a per-runtime alias still shows (MUL-5248).
   */
  machineTitle?: string;
  headerAction?: ReactNode;
}) {
  const { t } = useT("runtimes");
  const wsId = useWorkspaceId();
  const user = useAuthStore((s) => s.user);

  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: snapshot = [] } = useQuery(agentTaskSnapshotOptions(wsId));
  const { data: profiles = [] } = useQuery(runtimeProfileListOptions(wsId));

  const currentMember = user
    ? members.find((m) => m.user_id === user.id)
    : null;
  const isAdmin = currentMember
    ? currentMember.role === "owner" || currentMember.role === "admin"
    : false;

  const workloadIndex = useMemo(
    () => buildWorkloadIndex(agents, snapshot),
    [agents, snapshot],
  );

  const profileById = useMemo(() => {
    const map = new Map<string, RuntimeProfile>();
    for (const p of profiles) map.set(p.id, p);
    return map;
  }, [profiles]);

  const rows = useMemo<RuntimeRow[]>(() => {
    return runtimes.map((runtime) => {
      const profile = runtime.profile_id
        ? profileById.get(runtime.profile_id) ?? null
        : null;
      const isCustomRuntime = !!runtime.profile_id;
      return {
        runtime,
        profile,
        workload: workloadIndex.get(runtime.id) ?? EMPTY_WORKLOAD,
        canDelete: isCustomRuntime
          ? isAdmin && !!profile
          : !isPendingCustomRuntime(runtime) &&
            (isAdmin || (!!user && runtime.owner_id === user.id)),
      };
    });
  }, [runtimes, profileById, workloadIndex, isAdmin, user]);

  const columns = useMemo<ColumnDef<DataGridFeatures, RuntimeRow>[]>(
    () => [
      {
        id: "runtime",
        accessorFn: (row) => runtimeRowLabel(row.runtime, machineTitle ?? ""),
        header: t(($) => $.list.col_runtime),
        cell: ({ row }) => (
          <RuntimeNameCell
            runtime={row.original.runtime}
            machineTitle={machineTitle}
          />
        ),
        size: 220,
        enableSorting: false,
        enableHiding: false,
        meta: {
          headerTitle: t(($) => $.list.col_runtime),
        },
      },
      {
        id: "status",
        accessorFn: (row) => row.runtime.status,
        header: t(($) => $.list.col_health),
        cell: ({ row }) => (
          <HealthCell
            runtime={row.original.runtime}
            workload={row.original.workload}
            now={now}
          />
        ),
        size: 220,
        enableSorting: false,
        enableHiding: false,
        meta: {
          headerTitle: t(($) => $.list.col_health),
        },
      },
      {
        id: "cli",
        accessorFn: (row) => row.runtime.metadata?.version ?? "",
        header: t(($) => $.list.col_cli),
        cell: ({ row }) => <CliCell runtime={row.original.runtime} />,
        size: 220,
        enableSorting: false,
        enableHiding: false,
        meta: {
          headerTitle: t(($) => $.list.col_cli),
          fillWidth: true,
        },
      },
      {
        id: "actions",
        header: "",
        cell: ({ row }) => (
          <div
            className="flex justify-end"
            onClick={(event) => event.stopPropagation()}
          >
            <RuntimeRowMenu
              runtime={row.original.runtime}
              profile={row.original.profile}
              wsId={wsId}
              canDelete={row.original.canDelete}
            />
          </div>
        ),
        size: 48,
        enableSorting: false,
        enableHiding: false,
        enableResizing: false,
        meta: {
          cellClassName: "px-2!",
        },
      },
    ],
    [machineTitle, now, t, wsId],
  );

  const table = useTable({
    features: dataGridFeatures,
    data: rows,
    columns,
    getRowId: (row) => row.runtime.id,
  });

  return (
    <DataGrid
      table={table}
      recordCount={rows.length}
      tableLayout={{
        dense: true,
        rowBorder: true,
        headerBackground: true,
        width: "fixed",
      }}
      tableClassNames={{ bodyRow: "group/row" }}
    >
      <Frame className="w-full" dense>
        {headerAction ? (
          <FrameHeader className="flex-row items-center justify-end">
            {headerAction}
          </FrameHeader>
        ) : null}
        <FramePanel className="overflow-visible p-0!">
          <DataGridTable />
        </FramePanel>
      </Frame>
    </DataGrid>
  );
}
