"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, Bot, Plus } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import type {
  Agent,
  AgentRuntime,
  MemberWithUser,
} from "@orvilo/core/types";
import {
  type AgentActivity,
  agentRunCounts30dOptions,
  effectiveAccessScope,
  useWorkspaceActivityMap,
  useWorkspacePresenceMap,
  type AgentPresenceDetail,
} from "@orvilo/core/agents";
import {
  type AgentListFilters,
  useAgentsViewStore,
  AGENT_SCOPES,
  type AgentsScope,
  type AgentViewMode,
} from "@orvilo/core/agents/stores";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import {
  agentListOptions,
  memberListOptions,
} from "@orvilo/core/workspace/queries";
import { runtimeListOptions } from "@orvilo/core/runtimes";
import { Button } from "@orvilo/ui/components/ui/button";
import { MANAGEMENT_GRID_BOTTOM_CLEARANCE } from "../../common/management-grid";
import { useNavigation } from "../../navigation";
import { CollectionPageState } from "../../layout/collection-page";
import { ShellHeaderActions } from "../../layout/shell-header";
import {
  AgentListToolbar,
  countActiveFilterDimensions,
} from "./agent-list-toolbar";
import {
  AgentCard,
  AgentCardsLoadingSkeleton,
  AgentCreateCard,
  AGENT_CARD_GRID_CLASS,
  AGENT_CARD_GRID_GAP_CLASS,
} from "./agent-card";
import { AgentProfilePanel } from "./agent-profile-panel";
import { useLocale, useT } from "../../i18n";
import { matchesPinyin } from "../../editor/extensions/pinyin-match";
import { AgentTable, AgentTableSkeleton } from "./agent-table";

const AGENT_CARDS_PAGE_SIZE = 60;

export interface AgentListRow {
  agent: Agent;
  runtime: AgentRuntime | null;
  presence: AgentPresenceDetail | null;
  activity: AgentActivity | null;
  runCount: number;
  /** Days since the last bucket with runs; null = nothing in the window. */
  lastActiveDays: number | null;
  owner: MemberWithUser | null;
  isOwnedByMe: boolean;
  canManage: boolean;
}

// Most recent activity bucket with runs, as "days ago" (0 = today).
// Day-granularity by design — derived from the same 30-day buckets the
// detail page charts, no extra API.
function lastActiveDaysAgo(activity: AgentActivity | null): number | null {
  if (!activity) return null;
  for (let i = activity.buckets.length - 1; i >= 0; i--) {
    const bucket = activity.buckets[i];
    if (bucket && bucket.total > 0) return activity.buckets.length - 1 - i;
  }
  return null;
}

function matchesAgentSearch(row: AgentListRow, query: string): boolean {
  if (!query) return true;
  const { agent } = row;
  return (
    agent.name.toLowerCase().includes(query) ||
    matchesPinyin(agent.name, query) ||
    (agent.description?.toLowerCase().includes(query) ?? false) ||
    (agent.description ? matchesPinyin(agent.description, query) : false)
  );
}

/**
 * Pure row-filter predicate: returns true if the row matches all active
 * filter dimensions. Empty filter arrays are inactive (the row passes). The
 * `access` dimension derives its key via `effectiveAccessScope` so the column
 * and the filter share one derivation. Exported for testing — the page wires
 * it inside its `useMemo`.
 */
export function rowMatchesFilters(
  row: AgentListRow,
  filters: AgentListFilters,
  query: string,
): boolean {
  if (!matchesAgentSearch(row, query.trim().toLowerCase())) return false;
  if (
    filters.availability.length > 0 &&
    (!row.presence || !filters.availability.includes(row.presence.availability))
  ) {
    return false;
  }
  if (
    filters.devices.length > 0 &&
    !filters.devices.includes(row.agent.runtime_id)
  ) {
    return false;
  }
  if (
    filters.owners.length > 0 &&
    (!row.agent.owner_id || !filters.owners.includes(row.agent.owner_id))
  ) {
    return false;
  }
  if (
    filters.models.length > 0 &&
    !filters.models.includes(row.agent.model)
  ) {
    return false;
  }
  if (
    filters.access.length > 0 &&
    !filters.access.includes(
      effectiveAccessScope(
        row.agent.permission_mode,
        row.agent.invocation_targets,
      ),
    )
  ) {
    return false;
  }
  return true;
}

/**
 * Bulk-access dialog confirm-button enablement is centralized in
 * `@orvilo/core/agents` as `isAccessChangeReady` (MUL-3963). The dialog
 * consumes it; the picker also gates its internal Save button on the same
 * predicate (its own Save button is hidden via `hideFooter` in the bulk flow).
 */
import { isAccessChangeReady } from "@orvilo/core/agents";
import { AgentBatchToolbar } from "./agent-batch-toolbar";
export { isAccessChangeReady };

export interface AgentsPageProps {
  /** Desktop daemon identity used to label the current local device on cards. */
  localDaemonId?: string | null;
  localMachineName?: string | null;
  hasLocalMachine?: boolean;
}

// ---------------------------------------------------------------------------
// List states
// ---------------------------------------------------------------------------

function AgentsCreateAction({ onCreate }: { onCreate: () => void }) {
  const { t } = useT("agents");
  return (
    <ShellHeaderActions>
      <Button
        onClick={onCreate}
        size="sm"
        className="gap-1.5 shadow-2xs"
        data-testid="new-agent-header-btn"
      >
        <Plus className="size-3.5" data-icon="inline-start" />
        <span>{t(($) => $.page.new_agent)}</span>
      </Button>
    </ShellHeaderActions>
  );
}

function ListError({
  onCreate,
  listError,
  onRetry,
}: {
  onCreate: () => void;
  listError: unknown;
  onRetry: () => void;
}) {
  const { t } = useT("agents");
  return (
    <div className="flex flex-1 min-h-0 flex-col">
      <AgentsCreateAction onCreate={onCreate} />
      <CollectionPageState
        role="alert"
        tone="destructive"
        icon={AlertCircle}
        title={t(($) => $.page.list_load_failed)}
        description={
          listError instanceof Error
            ? listError.message
            : t(($) => $.page.list_load_failed_default)
        }
        actions={
          <Button type="button" variant="outline" size="sm" onClick={onRetry}>
            {t(($) => $.page.try_again)}
          </Button>
        }
      />
    </div>
  );
}

function EmptyState() {
  const { t } = useT("agents");
  return (
    <CollectionPageState
      icon={Bot}
      title={t(($) => $.empty.title)}
      description={t(($) => $.empty.description)}
    />
  );
}

// ---------------------------------------------------------------------------
// Batch toolbar — archive (with confirm; archiving cancels active tasks) and
// Page
// ---------------------------------------------------------------------------

export function AgentsPage({ localDaemonId }: AgentsPageProps = {}) {
  const { t } = useT("agents");
  const locale = useLocale();
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const navigation = useNavigation();
  const currentUser = useAuthStore((s) => s.user);

  const {
    data: agents = [],
    isLoading,
    error: listError,
    refetch: refetchList,
  } = useQuery(agentListOptions(wsId));
  const { data: runtimes = [] } = useQuery(
    runtimeListOptions(wsId),
  );
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: runCountsRaw = [], isPending: runCountsPending } = useQuery(
    agentRunCounts30dOptions(wsId),
  );
  const { byAgent: presenceMap, loading: presenceLoading } =
    useWorkspacePresenceMap(wsId);
  const { byAgent: activityMap, loading: activityLoading } =
    useWorkspaceActivityMap(wsId);

  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(
    new Set(),
  );
  const [profileAgentId, setProfileAgentId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [cardPage, setCardPage] = useState(0);
  const cardScrollRef = useRef<HTMLDivElement | null>(null);

  const rawScope = useAgentsViewStore((s) => s.scope);
  const scope = AGENT_SCOPES.includes(rawScope) ? rawScope : "mine";
  const rawViewMode = useAgentsViewStore((s) => s.viewMode);
  const viewMode: AgentViewMode =
    rawViewMode === "table" ? "table" : "cards";
  const setViewMode = useAgentsViewStore((s) => s.setViewMode);
  const setScope = useAgentsViewStore((s) => s.setScope);
  const sortField = useAgentsViewStore((s) => s.sortField);
  const sortDirection = useAgentsViewStore((s) => s.sortDirection);
  const hiddenColumns = useAgentsViewStore((s) => s.hiddenColumns);
  const filters = useAgentsViewStore((s) => s.filters);
  const handleSortFieldSelect = useAgentsViewStore((s) => s.setSortField);
  const setSortDirection = useAgentsViewStore((s) => s.setSortDirection);
  const toggleColumn = useAgentsViewStore((s) => s.toggleColumn);
  const toggleFilter = useAgentsViewStore((s) => s.toggleFilter);
  const clearFilters = useAgentsViewStore((s) => s.clearFilters);

  const runtimesById = useMemo(() => {
    const m = new Map<string, AgentRuntime>();
    for (const r of runtimes) m.set(r.id, r);
    return m;
  }, [runtimes]);

  const runCountsById = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of runCountsRaw) m.set(r.agent_id, r.run_count);
    return m;
  }, [runCountsRaw]);

  const membersById = useMemo(() => {
    const m = new Map<string, MemberWithUser>();
    for (const mem of members) m.set(mem.user_id, mem);
    return m;
  }, [members]);

  const isWorkspaceAdmin = useMemo(() => {
    if (!currentUser) return false;
    const me = members.find((m) => m.user_id === currentUser.id);
    return me?.role === "owner" || me?.role === "admin";
  }, [members, currentUser]);

  // Scope counts come from the FULL set (filters never affect them).
  // Archived ignores the ownership lens (see the view store comment).
  const scopeCounts = useMemo<Record<AgentsScope, number>>(() => {
    let mine = 0;
    let all = 0;
    let archived = 0;
    for (const a of agents) {
      if (a.archived_at) {
        archived++;
        continue;
      }
      all++;
      if (currentUser && a.owner_id === currentUser.id) mine++;
    }
    return { mine, all, archived };
  }, [agents, currentUser]);

  // Rows within the current scope, unfiltered, fully assembled — the
  // toolbar's option lists and the "n / total" denominator derive from
  // this; cells never pull their own queries.
  const scopeRows = useMemo<AgentListRow[]>(() => {
    const inScope = agents.filter((a) => {
      if (scope === "archived") return !!a.archived_at;
      if (a.archived_at) return false;
      if (scope === "mine") {
        return !!currentUser && a.owner_id === currentUser.id;
      }
      return true;
    });
    return inScope.map((agent) => {
      const isOwner = !!currentUser?.id && agent.owner_id === currentUser.id;
      const activity = activityMap.get(agent.id) ?? null;
      return {
        agent,
        runtime: runtimesById.get(agent.runtime_id) ?? null,
        presence: presenceMap.get(agent.id) ?? null,
        activity,
        runCount: runCountsById.get(agent.id) ?? 0,
        lastActiveDays: lastActiveDaysAgo(activity),
        owner: agent.owner_id ? membersById.get(agent.owner_id) ?? null : null,
        isOwnedByMe: isOwner,
        canManage: isWorkspaceAdmin || isOwner,
      };
    });
  }, [
    agents,
    scope,
    currentUser,
    runtimesById,
    membersById,
    presenceMap,
    activityMap,
    runCountsById,
    isWorkspaceAdmin,
  ]);

  const profileRow = useMemo(
    () =>
      profileAgentId === null
        ? null
        : (scopeRows.find((row) => row.agent.id === profileAgentId) ?? null),
    [profileAgentId, scopeRows],
  );

  useEffect(() => {
    if (profileAgentId !== null && profileRow === null) {
      setProfileAgentId(null);
    }
  }, [profileAgentId, profileRow]);

  // Visible rows: local search + filters, then sort. Table view uses the
  // ReUI grid's own search/status filter, so card-toolbar filters must not
  // silently shrink the dataset.
  const rows = useMemo<AgentListRow[]>(() => {
    const filtered =
      viewMode === "table"
        ? [...scopeRows]
        : scopeRows.filter((row) => rowMatchesFilters(row, filters, search));

    const dir = sortDirection === "asc" ? 1 : -1;
    filtered.sort((a, b) => {
      if (sortField === "name") {
        return a.agent.name.localeCompare(b.agent.name) * dir;
      }
      if (sortField === "runs") {
        return (a.runCount - b.runCount) * dir ||
          a.agent.name.localeCompare(b.agent.name);
      }
      if (sortField === "created") {
        return (
          (Date.parse(a.agent.created_at) - Date.parse(b.agent.created_at)) *
          dir
        );
      }
      // lastActive: smaller daysAgo = more recent. "desc" (the default)
      // means most recently active first; never-active rows sort last in
      // both directions. Run count breaks ties.
      const av = a.lastActiveDays ?? Number.POSITIVE_INFINITY;
      const bv = b.lastActiveDays ?? Number.POSITIVE_INFINITY;
      const byDays = sortDirection === "desc" ? av - bv : bv - av;
      return (
        byDays || b.runCount - a.runCount ||
        a.agent.name.localeCompare(b.agent.name)
      );
    });
    return filtered;
  }, [scopeRows, search, filters, sortField, sortDirection, viewMode]);

  useEffect(() => {
    setCardPage(0);
  }, [filters, scope, search, sortDirection, sortField]);

  const cardPageCount = Math.max(
    1,
    Math.ceil(rows.length / AGENT_CARDS_PAGE_SIZE),
  );
  useEffect(() => {
    setCardPage((page) => Math.min(page, cardPageCount - 1));
  }, [cardPageCount]);
  useEffect(() => {
    if (viewMode === "cards" && cardScrollRef.current) {
      cardScrollRef.current.scrollTop = 0;
    }
  }, [cardPage, viewMode]);
  const cardRows = rows.slice(
    cardPage * AGENT_CARDS_PAGE_SIZE,
    (cardPage + 1) * AGENT_CARDS_PAGE_SIZE,
  );

  const noMatchText = useMemo(() => {
    const query = search.trim();
    if (query) {
      if (scope === "archived") {
        return t(($) => $.no_matches.search_archived, { query });
      }
      if (countActiveFilterDimensions(filters) > 0) {
        return t(($) => $.no_matches.search_active_filtered, { query });
      }
      return t(($) => $.no_matches.search_active, { query });
    }
    if (scope === "archived") return t(($) => $.no_matches.no_archived);
    if (countActiveFilterDimensions(filters) > 0) {
      return t(($) => $.no_matches.no_filter_match);
    }
    return t(($) => $.no_matches.title);
  }, [filters, scope, search, t]);

  const selectedRows = rows.filter((row) => selectedIds.has(row.agent.id));

  if (listError) {
    return (
      <ListError
        onCreate={() => navigation.push(paths.newAgent())}
        listError={listError}
        onRetry={() => refetchList()}
      />
    );
  }

  const showEmpty = !isLoading && agents.length === 0;

  // The active sort field / availability filter reads columns that arrive in
  // separate queries from the main agent list (activity → lastActiveDays,
  // run-counts → runCount, presence → availability). Rendering real rows
  // before those land would sort on placeholder values (lastActiveDays
  // null→Infinity, runCount 0) and visibly re-order the list once each query
  // resolves. Gate the first real paint on exactly the auxiliary queries the
  // current sort / filter depends on — nothing for name/created, run-counts
  // for runs, activity + run-counts (its tiebreaker) for the default
  // lastActive, plus presence for every non-empty view because cards and the
  // table both render presence-dependent state. The queries still run in
  // parallel, so this defers the first paint by at most one extra round-trip
  // (shown as skeleton) and never serialises them. An empty workspace
  // (showEmpty) skips the gate so the empty state is never blocked on the
  // auxiliary queries.
  const needsRunCounts = sortField === "lastActive" || sortField === "runs";
  const needsActivity = sortField === "lastActive";
  const needsPresence = true;
  const listReady =
    (!needsActivity || !activityLoading) &&
    (!needsRunCounts || !runCountsPending) &&
    (!needsPresence || !presenceLoading);

  const openNewAgent = () => navigation.push(paths.newAgent());
  // The gallery card and the table toolbar already provide a create action.
  const showCreateActionInContent =
    !isLoading &&
    (showEmpty || (listReady && (viewMode === "table" || scope !== "archived")));

  return (
    // The list is a single surface. Opening an agent overlays a right-hand
    // sheet instead of splitting the gallery or navigating away.
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div className="relative flex min-w-0 flex-1 flex-col">
        {!showCreateActionInContent && <AgentsCreateAction onCreate={openNewAgent} />}

      {isLoading || (!showEmpty && !listReady) ? (
        viewMode === "table" ? (
          <AgentTableSkeleton />
        ) : (
        <div className="min-h-0 flex-1 overflow-y-auto @container">
          <div className="w-full p-4 sm:p-6">
            <AgentCardsLoadingSkeleton />
          </div>
        </div>
        )
      ) : showEmpty ? (
        <div className="min-h-0 flex-1 overflow-y-auto @container">
          <div className="w-full p-4 sm:p-6">
            <div className={`grid ${AGENT_CARD_GRID_GAP_CLASS} ${AGENT_CARD_GRID_CLASS}`}>
              <AgentCreateCard
                ariaLabel={t(($) => $.page.new_agent)}
                onClick={openNewAgent}
              />
            </div>
            <EmptyState />
          </div>
        </div>
      ) : (
        <>
          <AgentListToolbar
            scope={scope}
            onScopeChange={setScope}
            scopeCounts={scopeCounts}
            search={search}
            onSearchChange={setSearch}
            filters={filters}
            onToggleFilter={toggleFilter}
            onClearFilters={clearFilters}
            sortField={sortField}
            sortDirection={sortDirection}
            onSortFieldChange={handleSortFieldSelect}
            onSortDirectionChange={setSortDirection}
            hiddenColumns={hiddenColumns}
            onToggleColumn={toggleColumn}
            allRows={scopeRows}
            members={members}
            visibleCount={rows.length}
            viewMode={viewMode}
            onViewModeChange={setViewMode}
          />
          {viewMode === "table" ? (
            <AgentTable
              rows={rows}
              selectedIds={selectedIds}
              onSelectedIdsChange={setSelectedIds}
              noMatchText={noMatchText}
              locale={locale}
            />
          ) : (
            <div
              className="min-h-0 flex-1 overflow-y-auto @container"
              ref={cardScrollRef}
            >
              <div className="w-full p-4 sm:p-6">
                <div
                  className={`grid ${AGENT_CARD_GRID_GAP_CLASS} ${AGENT_CARD_GRID_CLASS}`}
                  style={{ paddingBottom: MANAGEMENT_GRID_BOTTOM_CLEARANCE }}
                >
                  {scope !== "archived" && (
                    <AgentCreateCard
                      ariaLabel={t(($) => $.page.new_agent)}
                      onClick={openNewAgent}
                    />
                  )}
                  {cardRows.map((row) => (
                    <AgentCard
                      key={row.agent.id}
                      row={row}
                      localDaemonId={localDaemonId}
                      onOpenSummary={() => setProfileAgentId(row.agent.id)}
                    />
                  ))}
                </div>
                {cardPageCount > 1 ? (
                  <nav
                    aria-label={t(($) => $.page.cards_pagination)}
                    className="flex items-center justify-center gap-3 pt-5"
                  >
                    <Button
                      disabled={cardPage === 0}
                      onClick={() => setCardPage((page) => page - 1)}
                      size="sm"
                      type="button"
                      variant="outline"
                    >
                      {t(($) => $.page.cards_previous)}
                    </Button>
                    <span
                      aria-live="polite"
                      className="text-caption text-muted-foreground"
                    >
                      {t(($) => $.page.cards_page, {
                        page: cardPage + 1,
                        total: cardPageCount,
                      })}
                    </span>
                    <Button
                      disabled={cardPage === cardPageCount - 1}
                      onClick={() => setCardPage((page) => page + 1)}
                      size="sm"
                      type="button"
                      variant="outline"
                    >
                      {t(($) => $.page.cards_next)}
                    </Button>
                  </nav>
                ) : null}
                {rows.length === 0 && (
                  <div className="py-16 text-center text-body text-muted-foreground">
                    {noMatchText}
                  </div>
                )}
              </div>
            </div>
          )}
        </>
      )}

      <AgentBatchToolbar
        rows={selectedRows}
        members={members}
        currentUserId={currentUser?.id ?? null}
        onClear={() => setSelectedIds(new Set())}
      />
      </div>
      {profileRow ? (
        <AgentProfilePanel
          row={profileRow}
          onClose={() => setProfileAgentId(null)}
        />
      ) : null}
    </div>
  );
}
