"use client";

import { useCallback, useMemo } from "react";
import { AlertCircle, Lock } from "lucide-react";
import { useTable, type ColumnDef, type RowSelectionState } from "@tanstack/react-table";
import type { Agent } from "@orvilo/core/types";
import {
  effectiveAccessScope,
  isAgentRuntimeBound,
  VISIBILITY_TOOLTIP,
} from "@orvilo/core/agents";
import {
  AGENT_DEFAULT_HIDDEN_COLUMNS,
  type AgentColumnKey,
  type AgentSortField,
} from "@orvilo/core/agents/stores";
import { runtimeDisplayLabel } from "@orvilo/core/runtimes";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { Checkbox } from "@orvilo/ui/components/ui/checkbox";
import {
  DataGrid,
  DataGridContainer,
  dataGridFeatures,
  type DataGridFeatures,
} from "@orvilo/ui/components/reui/data-grid/data-grid";
import { DataGridTable } from "@orvilo/ui/components/reui/data-grid/data-grid-table";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@orvilo/ui/components/ui/tooltip";
import { ActorAvatar } from "../../common/actor-avatar";
import { ManagementGrid } from "../../common/management-grid";
import { ProviderLogo } from "../../runtimes/components/provider-logo";
import { useT } from "../../i18n";
import { availabilityConfig } from "../presence";
import { AgentRowActions } from "./agent-row-actions";
import type { AgentListRow } from "./agents-page";

// Two-line rows: agents are identity-type entities (few, avatar + name), the
// documented exception to the single-line management-list rule.
export const AGENT_ROW_HEIGHT = 64;

// Responsiveness is TWO-ZONE and CONTAINER-query driven, exactly as the
// ListGrid convention this table replaces:
//   - >= @2xl: every user-enabled column renders.
//   - <  @2xl: a static core set of name + status.
// It is expressed in CSS, NEVER through TanStack's `columnVisibility`. That
// state belongs to the user's own column toggles; driving it from width would
// silently un-check a column the user enabled - the "dead toggle" bug this
// list shipped twice before.
const WIDE_ONLY = "hidden @2xl:table-cell";

// Column widths, in the order the grid lays them out. The wide-zone minimum
// width of the table derives from the same numbers, so the horizontal-scroll
// escape valve never disagrees with the tracks it is protecting.
const SELECT_WIDTH = 36;
const NAME_WIDTH = 260;
const ACTIONS_WIDTH = 44;
const COLUMN_WIDTHS: Record<AgentColumnKey, number> = {
  // Sized for the worst case "Online - 2 tasks" (~140px incl. padding).
  status: 144,
  owner: 144,
  // Fits the longest label "Specific people" (~120px incl. padding).
  access: 132,
  runtime: 144,
  lastActive: 120,
  runs: 88,
  model: 120,
  created: 104,
};

type SortState = {
  field: AgentSortField;
  direction: "asc" | "desc";
};

interface AgentTableProps {
  rows: AgentListRow[];
  selectedIds: ReadonlySet<string>;
  onToggleSelected: (id: string) => void;
  allSelected: boolean;
  someSelected: boolean;
  onToggleAll: () => void;
  sort: SortState;
  onSort: (field: AgentSortField) => void;
  isColVisible: (key: AgentColumnKey) => boolean;
  duplicateHref: (agent: Agent) => string;
  agentDetailHref: (id: string) => string;
  noMatchText: string;
  locale: string;
}

/**
 * The agents list in table form, on the shared ReUI DataGrid.
 */
export function AgentTable({
  rows,
  selectedIds,
  onToggleSelected,
  allSelected,
  someSelected,
  onToggleAll,
  sort,
  onSort,
  isColVisible,
  duplicateHref,
  agentDetailHref,
  noMatchText,
  locale,
}: AgentTableProps) {
  const { t } = useT("agents");
  const anySelected = allSelected || someSelected;

  const columns = useMemo<ColumnDef<DataGridFeatures, AgentListRow>[]>(() => {
    // A header cell is rendered through flexRender, so it must be a
    // component, not an element - hence the named wrapper per column.
    const sortableHeader = (
      field: AgentSortField,
      label: string,
      align: "start" | "end" = "start",
    ) => {
      const Header = () => (
        <SortHeader
          active={sort.field === field}
          align={align}
          direction={sort.direction}
          label={label}
          onSort={() => onSort(field)}
        />
      );
      Header.displayName = `AgentTableSortHeader(${field})`;
      return Header;
    };

    const SelectAllHeader = () => (
      <button
        type="button"
        aria-pressed={allSelected}
        onClick={onToggleAll}
        className={`-m-1.5 flex items-center p-1.5 ${
          anySelected
            ? ""
            : "opacity-0 transition-opacity group-hover/header:opacity-100"
        }`}
      >
        <Checkbox
          checked={allSelected}
          indeterminate={someSelected && !allSelected}
          tabIndex={-1}
          className="pointer-events-none"
        />
      </button>
    );
    SelectAllHeader.displayName = "AgentTableSelectAllHeader";

    const all: (ColumnDef<DataGridFeatures, AgentListRow> & {
      columnKey?: AgentColumnKey;
    })[] = [
      {
        id: "select",
        size: SELECT_WIDTH,
        meta: {
          headerClassName: WIDE_ONLY,
          cellClassName: WIDE_ONLY,
        },
        header: SelectAllHeader,
        cell: ({ row }) => {
          const checked = row.getIsSelected();
          return (
            <button
              type="button"
              aria-pressed={checked}
              // Stops the row-level navigation delegate on the scroll
              // container from also opening the agent.
              onClick={(event) => {
                event.stopPropagation();
                row.toggleSelected();
              }}
              className={`-m-1.5 flex items-center p-1.5 ${
                checked
                  ? ""
                  : "opacity-0 transition-opacity group-hover/row:opacity-100"
              }`}
            >
              <Checkbox
                checked={checked}
                tabIndex={-1}
                className="pointer-events-none"
              />
            </button>
          );
        },
      },
      {
        id: "name",
        size: NAME_WIDTH,
        header: sortableHeader("name", t(($) => $.columns.agent)),
        cell: ({ row }) => <NameCell row={row.original} />,
      },
      {
        id: "status",
        columnKey: "status",
        size: COLUMN_WIDTHS.status,
        header: t(($) => $.columns.status),
        cell: ({ row }) => <StatusCell row={row.original} />,
      },
      {
        id: "owner",
        columnKey: "owner",
        size: COLUMN_WIDTHS.owner,
        meta: { headerClassName: WIDE_ONLY, cellClassName: WIDE_ONLY },
        header: t(($) => $.columns.owner),
        cell: ({ row }) => <OwnerCell row={row.original} />,
      },
      {
        id: "access",
        columnKey: "access",
        size: COLUMN_WIDTHS.access,
        meta: { headerClassName: WIDE_ONLY, cellClassName: WIDE_ONLY },
        header: t(($) => $.columns.access),
        cell: ({ row }) => <AccessCell row={row.original} />,
      },
      {
        id: "runtime",
        columnKey: "runtime",
        size: COLUMN_WIDTHS.runtime,
        meta: { headerClassName: WIDE_ONLY, cellClassName: WIDE_ONLY },
        header: t(($) => $.columns.runtime),
        cell: ({ row }) => <RuntimeCell row={row.original} />,
      },
      {
        id: "lastActive",
        columnKey: "lastActive",
        size: COLUMN_WIDTHS.lastActive,
        meta: { headerClassName: WIDE_ONLY, cellClassName: WIDE_ONLY },
        header: sortableHeader("lastActive", t(($) => $.columns.last_active)),
        cell: ({ row }) => <LastActiveCell row={row.original} />,
      },
      {
        id: "runs",
        columnKey: "runs",
        size: COLUMN_WIDTHS.runs,
        meta: {
          headerClassName: `${WIDE_ONLY} text-right`,
          cellClassName: `${WIDE_ONLY} text-right`,
        },
        header: sortableHeader("runs", t(($) => $.columns.runs), "end"),
        cell: ({ row }) => (
          <span className="font-mono text-caption text-muted-foreground tabular-nums">
            {row.original.runCount.toLocaleString(locale)}
          </span>
        ),
      },
      {
        id: "model",
        columnKey: "model",
        size: COLUMN_WIDTHS.model,
        meta: { headerClassName: WIDE_ONLY, cellClassName: WIDE_ONLY },
        header: t(($) => $.columns.model),
        cell: ({ row }) => (
          <span className="block min-w-0 truncate text-caption text-muted-foreground">
            {row.original.agent.model || "—"}
          </span>
        ),
      },
      {
        id: "created",
        columnKey: "created",
        size: COLUMN_WIDTHS.created,
        meta: { headerClassName: WIDE_ONLY, cellClassName: WIDE_ONLY },
        header: sortableHeader("created", t(($) => $.columns.created)),
        cell: ({ row }) => (
          <span className="whitespace-nowrap text-caption text-muted-foreground tabular-nums">
            {new Date(row.original.agent.created_at).toLocaleDateString(locale)}
          </span>
        ),
      },
      {
        id: "actions",
        size: ACTIONS_WIDTH,
        meta: { headerClassName: "text-right", cellClassName: "text-right" },
        
        cell: ({ row }) => (
          <span
            onClick={(event) => event.stopPropagation()}
            className="inline-flex items-center"
          >
            <AgentRowActions
              agent={row.original.agent}
              presence={row.original.presence}
              canManage={row.original.canManage}
              duplicateHref={duplicateHref(row.original.agent)}
            />
          </span>
        ),
      },
    ];

    // A column the user has switched off is removed from the definition list
    // outright, so it takes no track. Width-driven hiding stays in CSS above.
    return all.filter(
      (column) => !column.columnKey || isColVisible(column.columnKey),
    );
  }, [
    allSelected,
    anySelected,
    duplicateHref,
    isColVisible,
    locale,
    onSort,
    onToggleAll,
    someSelected,
    sort.direction,
    sort.field,
    t,
  ]);

  // Selection stays owned by the page (the batch toolbar reads the same set),
  // so the grid gets it as controlled state and reports edits back one id at
  // a time. Feeding it in is what lights up the grid's own
  // `data-state="selected"` row chrome.
  const rowSelection = useMemo<RowSelectionState>(() => {
    const state: RowSelectionState = {};
    for (const id of selectedIds) state[id] = true;
    return state;
  }, [selectedIds]);

  const handleRowSelectionChange = useCallback(
    (updater: RowSelectionState | ((old: RowSelectionState) => RowSelectionState)) => {
      const next =
        typeof updater === "function" ? updater(rowSelection) : updater;
      for (const row of rows) {
        const id = row.agent.id;
        if (!!next[id] !== selectedIds.has(id)) onToggleSelected(id);
      }
    },
    [onToggleSelected, rowSelection, rows, selectedIds],
  );

  const table = useTable({
    features: dataGridFeatures,
    data: rows,
    columns,
    getRowId: (row) => row.agent.id,
    enableRowSelection: true,
    state: { rowSelection },
    onRowSelectionChange: handleRowSelectionChange,
    // Identity stays put while the wide zone scrolls: losing the avatar and
    // the name is what makes a scrolled row unreadable.
    initialState: { columnPinning: { start: ["select", "name"], end: [] } },
    // `rows` already IS the page; without this the grid slices it to the
    // default page size and renders ten rows.
    manualPagination: true,
  });

  // Row ids ARE agent ids (getRowId), so the delegated row navigation in
  // ManagementGrid resolves a row straight back to its detail route.
  const nameById = useMemo(
    () => new Map(rows.map((row) => [row.agent.id, row.agent.name])),
    [rows],
  );
  const hrefForRow = useCallback(
    (id: string) => {
      const title = nameById.get(id);
      if (title === undefined) return null;
      return { href: agentDetailHref(id), title };
    },
    [agentDetailHref, nameById],
  );

  // Wide-zone minimum width: below it the columns would crush instead of
  // scrolling. Derived from the same widths the tracks use.
  const minWidth =
    SELECT_WIDTH +
    NAME_WIDTH +
    ACTIONS_WIDTH +
    (Object.keys(COLUMN_WIDTHS) as AgentColumnKey[]).reduce(
      (sum, key) => sum + (isColVisible(key) ? COLUMN_WIDTHS[key] : 0),
      0,
    );

  return (
    <ManagementGrid
      table={table}
      recordCount={rows.length}
      emptyMessage={noMatchText}
      rowHeight={AGENT_ROW_HEIGHT}
      minWidth={minWidth}
      columnCount={columns.length}
      hrefForRow={hrefForRow}
    />
  );
}

// A sortable header keeps its own button so the sort affordance stays where it
// already was; the grid's own header chrome is not driving sort.
function SortHeader({
  active,
  align,
  direction,
  label,
  onSort,
}: {
  active: boolean;
  align: "start" | "end";
  direction: "asc" | "desc";
  label: string;
  onSort: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSort}
      aria-sort={
        active ? (direction === "asc" ? "ascending" : "descending") : undefined
      }
      className={`flex h-6 items-center rounded-md px-1.5 text-caption transition-colors ${
        align === "end" ? "-mr-1.5 ml-auto" : "-ml-1.5"
      } ${
        active
          ? "font-medium text-foreground"
          : "text-muted-foreground hover:bg-accent hover:text-accent-foreground"
      }`}
    >
      {label}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Loading skeleton
// ---------------------------------------------------------------------------

// Stable identity: a fresh [] on every render would re-seed the table core.
const EMPTY_ROWS: never[] = [];

function SkeletonHeaderNarrow() {
  return <Skeleton className="h-3 w-12" />;
}

function SkeletonHeaderWide() {
  return <Skeleton className="h-3 w-14" />;
}

// Placeholder rows in the grid's own skeleton mode: same columns, same widths,
// same row height, so the real list does not reflow when it lands. It runs on
// the NON-virtual body, the only one with a skeleton path — five fixed rows
// never need windowing anyway.
export function AgentTableSkeleton() {
  const columns = useMemo<ColumnDef<DataGridFeatures, never>[]>(() => {
    const visible = (key: AgentColumnKey) =>
      !AGENT_DEFAULT_HIDDEN_COLUMNS.includes(key);
    const all: (ColumnDef<DataGridFeatures, never> & {
      columnKey?: AgentColumnKey;
    })[] = [
      {
        id: "select",
        size: SELECT_WIDTH,
        meta: { headerClassName: WIDE_ONLY, cellClassName: WIDE_ONLY },
      },
      {
        id: "name",
        size: NAME_WIDTH,
        header: SkeletonHeaderNarrow,
        meta: {
          skeleton: (
            <span className="flex items-center gap-3">
              <Skeleton className="size-8 rounded-full" />
              <Skeleton className="h-3.5 w-32 max-w-full" />
            </span>
          ),
        },
      },
      ...(Object.keys(COLUMN_WIDTHS) as AgentColumnKey[])
        .filter(visible)
        .map((key) => ({
          id: key,
          columnKey: key,
          size: COLUMN_WIDTHS[key],
          header: SkeletonHeaderWide,
          meta: {
            headerClassName: key === "status" ? undefined : WIDE_ONLY,
            cellClassName: key === "status" ? undefined : WIDE_ONLY,
            skeleton: <Skeleton className="h-3 w-16" />,
          },
        })),
      {
        id: "actions",
        size: ACTIONS_WIDTH,
      },
    ];
    return all;
  }, []);

  const table = useTable({
    features: dataGridFeatures,
    data: EMPTY_ROWS,
    columns,
    initialState: { pagination: { pageIndex: 0, pageSize: 5 } },
  });

  return (
    <div className="min-h-0 flex-1 overflow-auto @container" aria-busy="true">
      <DataGrid
        table={table}
        recordCount={0}
        isLoading
        loadingMode="skeleton"
        tableLayout={{
          width: "fixed",
          headerSticky: true,
          headerBackground: true,
          rowBorder: true,
        }}
        tableClassNames={{ bodyRow: "h-16 hover:bg-transparent" }}
      >
        <DataGridContainer>
          <DataGridTable />
        </DataGridContainer>
      </DataGrid>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------

// Two-line identity cell: avatar left, name right. The documented exception to
// the single-line rule — agents are few, so this is the "team roster" form.
function NameCell({ row }: { row: AgentListRow }) {
  const { t } = useT("agents");
  const { agent, isOwnedByMe } = row;
  const isArchived = !!agent.archived_at;
  const isPrivate = agent.visibility === "private";
  return (
    <div className="flex min-w-0 items-center gap-3">
      <ActorAvatar
        actorType="agent"
        actorId={agent.id}
        size="lg"
        className={`shrink-0 ${isArchived ? "opacity-50 grayscale" : ""}`}
        showStatusDot
      />
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <span
          className={`min-w-0 truncate text-body font-medium ${
            isArchived ? "text-muted-foreground" : ""
          }`}
        >
          {agent.name}
        </span>
        {isPrivate && !isArchived && (
          <Tooltip>
            <TooltipTrigger
              render={
                <Lock className="size-3.5 shrink-0 text-faint-foreground" />
              }
            />
            <TooltipContent>{VISIBILITY_TOOLTIP.private}</TooltipContent>
          </Tooltip>
        )}
        {isOwnedByMe && (
          <span className="shrink-0 rounded bg-muted px-1 text-micro font-medium text-muted-foreground">
            {t(($) => $.row.you)}
          </span>
        )}
      </div>
    </div>
  );
}

// Availability as a chip, with the workload folded in as a suffix
// ("Online · 2 tasks") — a 0-2 integer doesn't earn its own column. The chip
// variant comes from the shared availability config, so the list, the detail
// header and the hover card can never disagree on tone.
function StatusCell({ row }: { row: AgentListRow }) {
  const { t } = useT("agents");
  const { agent, presence } = row;
  if (agent.archived_at) {
    return (
      <Badge variant="secondary" size="sm">
        {t(($) => $.row.archived)}
      </Badge>
    );
  }
  if (!isAgentRuntimeBound(agent)) {
    return (
      <Badge variant="warning-light" size="sm">
        <AlertCircle />
        {t(($) => $.row.needs_runtime)}
      </Badge>
    );
  }
  if (!presence) {
    return <span className="text-caption text-faint-foreground">—</span>;
  }
  const visual = availabilityConfig[presence.availability];
  const active = presence.runningCount + presence.queuedCount;
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      <Badge variant={visual.badgeVariant} size="sm">
        {t(($) => $.availability[presence.availability])}
      </Badge>
      {active > 0 && (
        <span className="truncate text-caption text-muted-foreground">
          {t(($) => $.row.task_count, { count: active })}
        </span>
      )}
    </span>
  );
}

// Owner = the agent's owner_id, set to the creator at creation and never
// transferred (so owner ≡ creator). It carries management rights, so the
// column is "Owner", not "Created by".
function OwnerCell({ row }: { row: AgentListRow }) {
  const { agent, owner } = row;
  if (!agent.owner_id) {
    return <span className="text-caption text-faint-foreground">—</span>;
  }
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      <ActorAvatar actorType="member" actorId={agent.owner_id} size="sm" />
      <span className="min-w-0 truncate text-caption text-muted-foreground">
        {owner?.name ?? agent.owner_id.slice(0, 8)}
      </span>
    </span>
  );
}

// Effective access scope derived from permission_mode + invocation_targets
// (not the lossy derived `visibility`). Text label, not icon-only, so screen
// readers announce the scope.
export function AccessCell({ row }: { row: AgentListRow }) {
  const { t } = useT("agents");
  const scope = useMemo(
    () =>
      effectiveAccessScope(
        row.agent.permission_mode,
        row.agent.invocation_targets,
      ),
    [row.agent.permission_mode, row.agent.invocation_targets],
  );
  const label = t(($) =>
    scope === "workspace"
      ? $.access.scope_labels.workspace
      : scope === "specific-people"
        ? $.access.scope_labels.specific_people
        : $.access.scope_labels.owner_only,
  );
  return (
    <span className="min-w-0 truncate text-caption text-muted-foreground">
      {label}
    </span>
  );
}

function RuntimeCell({ row }: { row: AgentListRow }) {
  const { t } = useT("agents");
  if (!isAgentRuntimeBound(row.agent)) {
    return (
      <Badge variant="warning-light" size="sm">
        {t(($) => $.row.needs_runtime)}
      </Badge>
    );
  }
  const runtime = row.runtime;
  if (!runtime) {
    return <span className="text-caption text-faint-foreground">—</span>;
  }
  // Provider mark before the label: scanning this column for "which of these
  // run on Codex" is a shape match, not a read.
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <ProviderLogo
        provider={runtime.provider}
        className="size-3.5 shrink-0"
      />
      <span className="min-w-0 truncate text-caption text-muted-foreground">
        {runtimeDisplayLabel(runtime)}
      </span>
    </span>
  );
}

function LastActiveCell({ row }: { row: AgentListRow }) {
  const { t } = useT("agents");
  const days = row.lastActiveDays;
  if (days === null) {
    if (row.agent.archived_at) {
      return <span className="text-caption text-faint-foreground">—</span>;
    }
    // The full sentence ("No activity (30d)") does not fit the track, so the
    // cell carries the short form and the window moves to a tooltip.
    return (
      <Tooltip>
        <TooltipTrigger
          render={
            <span className="truncate text-caption text-muted-foreground">
              {t(($) => $.last_active.none_short)}
            </span>
          }
        />
        <TooltipContent>{t(($) => $.last_active.none)}</TooltipContent>
      </Tooltip>
    );
  }
  return (
    <span className="whitespace-nowrap text-caption text-muted-foreground tabular-nums">
      {days === 0
        ? t(($) => $.last_active.today)
        : t(($) => $.last_active.days_ago, { count: days })}
    </span>
  );
}
