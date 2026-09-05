"use client";

import { useEffect, useRef } from "react";
import { create } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import { createJSONStorage, persist } from "zustand/middleware";
import type { IssueStatus, IssueStatusCategory, IssuePriority, PropertyFilterValue } from "../../types";
import { createWorkspaceAwareStorage, registerForWorkspaceRehydration } from "../../platform/workspace-storage";
import { defaultStorage } from "../../platform/storage";

export type ViewMode = "board" | "list" | "table" | "gantt" | "swimlane";
export type GanttZoom = "day" | "week" | "month";
/**
 * Board grouping. Besides the three built-ins, a select-type custom property
 * groups columns by its options via the `property:<definitionId>` form.
 * Persisted values may reference a since-archived definition — consumers must
 * fall back to "status" when the definition can't be resolved.
 */
export type IssueGrouping =
  | "status"
  | "executor"
  | "project"
  | `property:${string}`;
export type SwimlaneGrouping = "parent" | "project" | "executor";
/**
 * Sort key. `property:<definitionId>` is resolved server-side against the
 * active property catalog; stale or unsupported definitions degrade to
 * position order.
 */
export type SortField =
  | "position"
  | "status"
  | "priority"
  | "start_date"
  | "due_date"
  | "created_at"
  | "updated_at"
  | "title"
  | `property:${string}`;
export type SortDirection = "asc" | "desc";
export type IssueDateField = "created_at" | "updated_at";

export type TableSystemColumnKey =
  | "title"
  | "identifier"
  | "status"
  | "priority"
  | "executor"
  | "labels"
  | "project"
  | "start_date"
  | "due_date"
  | "created_at"
  | "updated_at"
  | "child_progress"
  | "creator";
export type TableColumnKey = TableSystemColumnKey | `property:${string}`;
export interface TableColumnConfig {
  key: TableColumnKey;
  width?: number;
}
export type TableGrouping =
  | "none"
  | "status"
  | "executor"
  | "project"
  | `property:${string}`;
export type TableCalculation = "none" | "sum" | "average" | "count";

export const TABLE_SYSTEM_COLUMNS: readonly TableSystemColumnKey[] = [
  "title",
  "identifier",
  "status",
  "priority",
  "executor",
  "labels",
  "project",
  "start_date",
  "due_date",
  "created_at",
  "updated_at",
  "child_progress",
  "creator",
];

export const DEFAULT_TABLE_COLUMNS: readonly TableColumnConfig[] = [
  { key: "title", width: 360 },
  { key: "status", width: 150 },
  { key: "priority", width: 130 },
  { key: "executor", width: 180 },
  { key: "due_date", width: 140 },
  { key: "labels", width: 220 },
];

export interface IssueDateFilter {
  field: IssueDateField;
  from: string;
  to: string;
}

export const SWIMLANE_GROUPINGS: SwimlaneGrouping[] = ["parent", "project", "executor"];

export interface CardProperties {
  priority: boolean;
  description: boolean;
  executor: boolean;
  startDate: boolean;
  dueDate: boolean;
  project: boolean;
  childProgress: boolean;
  labels: boolean;
}

export interface ActorFilterValue {
  type: "member" | "agent" | "team";
  id: string;
}

/** The nine query-defining filter fields as one value — what a saved view
 *  fixes, and what resets restore. */
export interface FilterSnapshot {
  statusFilters: IssueStatus[];
  priorityFilters: IssuePriority[];
  executorFilters: ActorFilterValue[];
  includeNoExecutor: boolean;
  creatorFilters: ActorFilterValue[];
  projectFilters: string[];
  includeNoProject: boolean;
  labelFilters: string[];
  propertyFilters: Record<string, PropertyFilterValue[]>;
}

/** Filter-bar chip dimensions. Date is excluded: `dateFilter` lives outside
 *  the persisted slice and clears through `setDateFilter(null)`. */
export type FilterDimension =
  | "status"
  | "priority"
  | "executor"
  | "creator"
  | "project"
  | "label"
  | `property:${string}`;

export const PROPERTY_VIEW_PREFIX = "property:";

export function propertyIdFromViewKey(key: string): string | null {
  return key.startsWith(PROPERTY_VIEW_PREFIX) ? key.slice(PROPERTY_VIEW_PREFIX.length) : null;
}

export type StaticSortField = Exclude<SortField, `property:${string}`>;
export type StaticIssueGrouping = Exclude<IssueGrouping, `property:${string}`>;

export const SORT_OPTIONS: { value: StaticSortField; label: string }[] = [
  { value: "position", label: "Manual" },
  { value: "status", label: "Status" },
  { value: "priority", label: "Priority" },
  { value: "start_date", label: "Start date" },
  { value: "due_date", label: "Due date" },
  { value: "created_at", label: "Created date" },
  { value: "updated_at", label: "Updated date" },
  { value: "title", label: "Title" },
];

export const GROUPING_OPTIONS: { value: StaticIssueGrouping; label: string }[] = [
  { value: "status", label: "Status" },
  { value: "executor", label: "Executor" },
  { value: "project", label: "Project" },
];

export const CARD_PROPERTY_OPTIONS: { key: keyof CardProperties; label: string }[] = [
  { key: "priority", label: "Priority" },
  { key: "description", label: "Description" },
  { key: "executor", label: "Executor" },
  { key: "startDate", label: "Start date" },
  { key: "dueDate", label: "Due date" },
  { key: "project", label: "Project" },
  { key: "labels", label: "Labels" },
  { key: "childProgress", label: "Sub-issue progress" },
];

export interface IssueViewState {
  viewMode: ViewMode;
  grouping: IssueGrouping;
  statusFilters: IssueStatus[];
  priorityFilters: IssuePriority[];
  executorFilters: ActorFilterValue[];
  includeNoExecutor: boolean;
  creatorFilters: ActorFilterValue[];
  projectFilters: string[];
  includeNoProject: boolean;
  labelFilters: string[];
  /**
   * Custom-property filters: definition id → selected values (checkbox
   * definitions use the pseudo-options "true"/"false"; scalars hold the
   * committed value as a bare string, or an operator object per
   * `PropertyFilterValue`, plus the "__none__" sentinel). Empty array = no
   * filter for that definition; matching is OR within a definition and AND
   * across definitions, mirroring the other filter groups.
   */
  propertyFilters: Record<string, PropertyFilterValue[]>;
  dateFilter: IssueDateFilter | null;
  // When true, the list only shows issues that currently have at least one
  // agent task in `running` status. Drives the workspace "agents working"
  // quick filter chip in the issues header. Not persisted across reloads —
  // running state changes second-to-second, a persisted toggle would let
  // users return to an empty list with no obvious cause.
  agentRunningFilter: boolean;
  sortBy: SortField;
  sortDirection: SortDirection;
  cardProperties: CardProperties;
  /** Custom property definition ids whose values render on board/list cards. */
  cardPropertyIds: string[];
  // When false, issues that have a parent (sub-issues) are hidden from the
  // board / list / swimlane so users can focus on top-level parent issues.
  // Purely a display filter — it never touches the parent/child relationship.
  showSubIssues: boolean;
  listCollapsedStatuses: IssueStatusCategory[];
  /**
   * Board / list columns the user hid, as CATEGORIES.
   *
   * Column visibility used to be expressed by writing the surviving statuses
   * into `statusFilters`, which stopped being correct once a category can hold
   * more than one status: hiding Backlog wrote the other 6 built-in keys and
   * so silently filtered out every CUSTOM status too. Display state and the
   * exact-key filter are different questions and now have different fields.
   * (MUL-6243)
   */
  hiddenStatusCategories: IssueStatusCategory[];
  ganttZoom: GanttZoom;
  ganttShowCompleted: boolean;
  /** Active swimlane grouping dimension. */
  swimlaneGrouping: SwimlaneGrouping;
  /** Persisted lane order, keyed by grouping. Entries are raw lane ids
   *  (parent issue id, project id, or `<executorType>:<executorId>`). */
  swimlaneOrders: Record<SwimlaneGrouping, string[]>;
  /** Persisted collapsed lanes, keyed by grouping. Same id space as
   *  `swimlaneOrders`, plus the sentinel `"none"` for the pinned
   *  no-X lane and `"__orphans__"` for the parent-grouping fallback. */
  collapsedSwimlanes: Record<SwimlaneGrouping, string[]>;
  /** Ordered table columns. Title is mandatory and normalized to the front. */
  tableColumns: TableColumnConfig[];
  tableGrouping: TableGrouping;
  tableCollapsedGroups: string[];
  tableCollapsedParents: string[];
  tableHierarchy: boolean;
  tableCalculation: TableCalculation;
  setViewMode: (mode: ViewMode) => void;
  setGanttZoom: (zoom: GanttZoom) => void;
  toggleGanttShowCompleted: () => void;
  setGrouping: (grouping: IssueGrouping) => void;
  toggleStatusFilter: (status: IssueStatus) => void;
  togglePriorityFilter: (priority: IssuePriority) => void;
  toggleExecutorFilter: (value: ActorFilterValue) => void;
  toggleNoExecutor: () => void;
  toggleCreatorFilter: (value: ActorFilterValue) => void;
  toggleProjectFilter: (projectId: string) => void;
  toggleNoProject: () => void;
  toggleLabelFilter: (labelId: string) => void;
  togglePropertyFilter: (propertyId: string, optionId: string) => void;
  /** Replace a property's full filter value set (used by scalar value inputs
   *  for text/number/date/url, which build the array including "__none__"). */
  setPropertyFilterValues: (propertyId: string, optionIds: PropertyFilterValue[]) => void;
  setDateFilter: (filter: IssueDateFilter | null) => void;
  toggleAgentRunningFilter: () => void;
  hideStatus: (category: IssueStatusCategory) => void;
  showStatus: (category: IssueStatusCategory) => void;
  clearFilters: () => void;
  /** Clear one filter dimension (a filter-bar chip). `property:<id>` clears
   *  that definition's entry only. Paired boolean flags (no-executor /
   *  no-project) clear with their dimension. */
  clearFilterDimension: (dimension: FilterDimension) => void;
  /** Replace every filter field at once — how "reset inside a saved view"
   *  returns to the view's own conditions instead of to nothing. */
  resetFiltersTo: (snapshot: FilterSnapshot) => void;
  setSortBy: (field: SortField) => void;
  setSortDirection: (dir: SortDirection) => void;
  toggleCardProperty: (key: keyof CardProperties) => void;
  toggleCardPropertyId: (propertyId: string) => void;
  toggleShowSubIssues: () => void;
  toggleListCollapsed: (category: IssueStatusCategory) => void;
  setSwimlaneGrouping: (grouping: SwimlaneGrouping) => void;
  /** Update the lane order for the currently active swimlane grouping. */
  setSwimlaneOrder: (order: string[]) => void;
  /** Toggle a lane key in the currently active swimlane grouping. */
  toggleSwimlaneCollapsed: (key: string) => void;
  toggleTableColumn: (key: TableColumnKey) => void;
  reorderTableColumn: (active: TableColumnKey, over: TableColumnKey) => void;
  setTableColumnWidth: (key: TableColumnKey, width?: number) => void;
  setTableGrouping: (grouping: TableGrouping) => void;
  toggleTableGroupCollapsed: (key: string) => void;
  toggleTableParentCollapsed: (issueId: string) => void;
  toggleTableHierarchy: () => void;
  setTableCalculation: (calculation: TableCalculation) => void;
}

export const viewStoreSlice = (set: StoreApi<IssueViewState>["setState"]): IssueViewState => ({
  viewMode: "board",
  grouping: "status",
  statusFilters: [],
  priorityFilters: [],
  executorFilters: [],
  includeNoExecutor: false,
  creatorFilters: [],
  projectFilters: [],
  includeNoProject: false,
  labelFilters: [],
  propertyFilters: {},
  dateFilter: null,
  agentRunningFilter: false,
  sortBy: "position",
  sortDirection: "asc",
  cardProperties: {
    priority: true,
    // Board cards stay dense unless the user opts into a description preview.
    description: false,
    executor: true,
    startDate: true,
    dueDate: true,
    project: true,
    childProgress: true,
    labels: true,
  },
  cardPropertyIds: [],
  showSubIssues: true,
  listCollapsedStatuses: [],
  hiddenStatusCategories: [],
  ganttZoom: "week",
  ganttShowCompleted: false,
  swimlaneGrouping: "executor",
  swimlaneOrders: { parent: [], project: [], executor: [] },
  collapsedSwimlanes: { parent: [], project: [], executor: [] },
  tableColumns: DEFAULT_TABLE_COLUMNS.map((column) => ({ ...column })),
  tableGrouping: "none",
  tableCollapsedGroups: [],
  tableCollapsedParents: [],
  tableHierarchy: true,
  tableCalculation: "none",

  setViewMode: (mode) => set({ viewMode: mode }),
  setGanttZoom: (zoom) => set({ ganttZoom: zoom }),
  toggleGanttShowCompleted: () =>
    set((state) => ({ ganttShowCompleted: !state.ganttShowCompleted })),
  setGrouping: (grouping) => set({ grouping }),
  toggleStatusFilter: (status) =>
    set((state) => ({
      statusFilters: state.statusFilters.includes(status)
        ? state.statusFilters.filter((s) => s !== status)
        : [...state.statusFilters, status],
    })),
  togglePriorityFilter: (priority) =>
    set((state) => ({
      priorityFilters: state.priorityFilters.includes(priority)
        ? state.priorityFilters.filter((p) => p !== priority)
        : [...state.priorityFilters, priority],
    })),
  toggleExecutorFilter: (value) =>
    set((state) => {
      const exists = state.executorFilters.some(
        (f) => f.type === value.type && f.id === value.id,
      );
      return {
        executorFilters: exists
          ? state.executorFilters.filter(
              (f) => !(f.type === value.type && f.id === value.id),
            )
          : [...state.executorFilters, value],
      };
    }),
  toggleNoExecutor: () =>
    set((state) => ({ includeNoExecutor: !state.includeNoExecutor })),
  toggleCreatorFilter: (value) =>
    set((state) => {
      const exists = state.creatorFilters.some(
        (f) => f.type === value.type && f.id === value.id,
      );
      return {
        creatorFilters: exists
          ? state.creatorFilters.filter(
              (f) => !(f.type === value.type && f.id === value.id),
            )
          : [...state.creatorFilters, value],
      };
    }),
  toggleProjectFilter: (projectId) =>
    set((state) => ({
      projectFilters: state.projectFilters.includes(projectId)
        ? state.projectFilters.filter((id) => id !== projectId)
        : [...state.projectFilters, projectId],
    })),
  toggleNoProject: () =>
    set((state) => ({ includeNoProject: !state.includeNoProject })),
  toggleLabelFilter: (labelId) =>
    set((state) => ({
      labelFilters: state.labelFilters.includes(labelId)
        ? state.labelFilters.filter((id) => id !== labelId)
        : [...state.labelFilters, labelId],
    })),
  togglePropertyFilter: (propertyId, optionId) =>
    set((state) => {
      const current = state.propertyFilters[propertyId] ?? [];
      const next = current.includes(optionId)
        ? current.filter((id) => id !== optionId)
        : [...current, optionId];
      const propertyFilters = { ...state.propertyFilters };
      if (next.length === 0) delete propertyFilters[propertyId];
      else propertyFilters[propertyId] = next;
      return { propertyFilters };
    }),
  setPropertyFilterValues: (propertyId, optionIds) =>
    set((state) => {
      const propertyFilters = { ...state.propertyFilters };
      if (optionIds.length === 0) delete propertyFilters[propertyId];
      else propertyFilters[propertyId] = optionIds;
      return { propertyFilters };
    }),
  setDateFilter: (filter) => set({ dateFilter: filter }),
  toggleAgentRunningFilter: () =>
    set((state) => ({ agentRunningFilter: !state.agentRunningFilter })),
  hideStatus: (category) =>
    set((state) =>
      state.hiddenStatusCategories.includes(category)
        ? state
        : { hiddenStatusCategories: [...state.hiddenStatusCategories, category] },
    ),
  showStatus: (category) =>
    set((state) => ({
      hiddenStatusCategories: state.hiddenStatusCategories.filter((c) => c !== category),
    })),
  clearFilters: () =>
    set({
      statusFilters: [],
      priorityFilters: [],
      executorFilters: [],
      includeNoExecutor: false,
      creatorFilters: [],
      projectFilters: [],
      includeNoProject: false,
      labelFilters: [],
      propertyFilters: {},
      dateFilter: null,
      agentRunningFilter: false,
      // Reset restores every column, matching what it did when hiding a column
      // was expressed as a status filter.
      hiddenStatusCategories: [],
    }),
  resetFiltersTo: (snapshot) => set({ ...snapshot }),
  clearFilterDimension: (dimension) =>
    set((state) => {
      switch (dimension) {
        case "status":
          return { statusFilters: [] };
        case "priority":
          return { priorityFilters: [] };
        case "executor":
          return { executorFilters: [], includeNoExecutor: false };
        case "creator":
          return { creatorFilters: [] };
        case "project":
          return { projectFilters: [], includeNoProject: false };
        case "label":
          return { labelFilters: [] };
        default: {
          const propertyId = propertyIdFromViewKey(dimension);
          if (!propertyId || !(propertyId in state.propertyFilters)) return state;
          const propertyFilters = { ...state.propertyFilters };
          delete propertyFilters[propertyId];
          return { propertyFilters };
        }
      }
    }),
  setSortBy: (field) => set({ sortBy: field }),
  setSortDirection: (dir) => set({ sortDirection: dir }),
  toggleCardProperty: (key) =>
    set((state) => ({
      cardProperties: {
        ...state.cardProperties,
        [key]: !state.cardProperties[key],
      },
    })),
  toggleCardPropertyId: (propertyId) =>
    set((state) => ({
      cardPropertyIds: state.cardPropertyIds.includes(propertyId)
        ? state.cardPropertyIds.filter((id) => id !== propertyId)
        : [...state.cardPropertyIds, propertyId],
    })),
  toggleShowSubIssues: () =>
    set((state) => ({ showSubIssues: !state.showSubIssues })),
  toggleListCollapsed: (status) =>
    set((state) => ({
      listCollapsedStatuses: state.listCollapsedStatuses.includes(status)
        ? state.listCollapsedStatuses.filter((s) => s !== status)
        : [...state.listCollapsedStatuses, status],
    })),
  setSwimlaneGrouping: (grouping) => set({ swimlaneGrouping: grouping }),
  setSwimlaneOrder: (order) =>
    set((state) => ({
      swimlaneOrders: { ...state.swimlaneOrders, [state.swimlaneGrouping]: order },
    })),
  toggleSwimlaneCollapsed: (key) =>
    set((state) => {
      const grouping = state.swimlaneGrouping;
      const current = state.collapsedSwimlanes[grouping];
      const next = current.includes(key)
        ? current.filter((k) => k !== key)
        : [...current, key];
      return {
        collapsedSwimlanes: { ...state.collapsedSwimlanes, [grouping]: next },
      };
    }),
  toggleTableColumn: (key) =>
    set((state) => {
      if (key === "title") return state;
      const exists = state.tableColumns.some((column) => column.key === key);
      return {
        tableColumns: exists
          ? state.tableColumns.filter((column) => column.key !== key)
          : [...state.tableColumns, { key }],
      };
    }),
  reorderTableColumn: (active, over) =>
    set((state) => {
      if (active === "title" || over === "title" || active === over) return state;
      const from = state.tableColumns.findIndex((column) => column.key === active);
      const to = state.tableColumns.findIndex((column) => column.key === over);
      if (from < 0 || to < 0) return state;
      const tableColumns = [...state.tableColumns];
      const [moved] = tableColumns.splice(from, 1);
      if (!moved) return state;
      tableColumns.splice(to, 0, moved);
      return { tableColumns };
    }),
  setTableColumnWidth: (key, width) =>
    set((state) => ({
      tableColumns: state.tableColumns.map((column) =>
        column.key === key
          ? { ...column, ...(width === undefined ? { width: undefined } : { width }) }
          : column,
      ),
    })),
  setTableGrouping: (tableGrouping) => set({ tableGrouping }),
  toggleTableGroupCollapsed: (key) =>
    set((state) => ({
      tableCollapsedGroups: state.tableCollapsedGroups.includes(key)
        ? state.tableCollapsedGroups.filter((item) => item !== key)
        : [...state.tableCollapsedGroups, key],
    })),
  toggleTableParentCollapsed: (issueId) =>
    set((state) => ({
      tableCollapsedParents: state.tableCollapsedParents.includes(issueId)
        ? state.tableCollapsedParents.filter((id) => id !== issueId)
        : [...state.tableCollapsedParents, issueId],
    })),
  toggleTableHierarchy: () =>
    set((state) => ({ tableHierarchy: !state.tableHierarchy })),
  setTableCalculation: (tableCalculation) => set({ tableCalculation }),
});

export const viewStorePersistOptions = (name: string) => ({
  name,
  storage: createJSONStorage(() => createWorkspaceAwareStorage(defaultStorage)),
  partialize: (state: IssueViewState) => ({
    // NOTE: `agentRunningFilter` is intentionally NOT persisted — running
    // state changes second-to-second, and a stored toggle would let users
    // return to an unexplained empty list. Keep it ephemeral. See the
    // field comment on IssueViewState.
    // `dateFilter` is also intentionally not persisted: relative presets such
    // as Today would otherwise become stale after a calendar-day rollover.
    viewMode: state.viewMode,
    grouping: state.grouping,
    statusFilters: state.statusFilters,
    priorityFilters: state.priorityFilters,
    executorFilters: state.executorFilters,
    includeNoExecutor: state.includeNoExecutor,
    creatorFilters: state.creatorFilters,
    projectFilters: state.projectFilters,
    includeNoProject: state.includeNoProject,
    labelFilters: state.labelFilters,
    propertyFilters: state.propertyFilters,
    sortBy: state.sortBy,
    sortDirection: state.sortDirection,
    cardProperties: state.cardProperties,
    cardPropertyIds: state.cardPropertyIds,
    showSubIssues: state.showSubIssues,
    listCollapsedStatuses: state.listCollapsedStatuses,
    hiddenStatusCategories: state.hiddenStatusCategories,
    ganttZoom: state.ganttZoom,
    ganttShowCompleted: state.ganttShowCompleted,
    swimlaneGrouping: state.swimlaneGrouping,
    swimlaneOrders: state.swimlaneOrders,
    collapsedSwimlanes: state.collapsedSwimlanes,
    tableColumns: state.tableColumns,
    tableGrouping: state.tableGrouping,
    tableCollapsedGroups: state.tableCollapsedGroups,
    tableCollapsedParents: state.tableCollapsedParents,
    tableHierarchy: state.tableHierarchy,
    tableCalculation: state.tableCalculation,
  }),
  // Default Zustand merge is shallow, so a persisted `cardProperties` snapshot
  // saved before a new toggle was introduced wins entirely and the new key is
  // missing — the dropdown switch then reads `undefined` and renders unchecked
  // even though defaults treat it as on. Deep-merge `cardProperties` so newly
  // added toggles inherit their default value for existing users.
  merge: mergeViewStatePersisted,
});

/**
 * Reusable persist `merge` for view-state stores. Generic over T so the same
 * deep-merge for `cardProperties` works for both the issues view store and
 * the my-issues view store (which extends IssueViewState).
 */
export function mergeViewStatePersisted<T extends IssueViewState>(
  persisted: unknown,
  current: T,
): T {
  const isRecord = (value: unknown): value is Record<string, unknown> =>
    value !== null && typeof value === "object" && !Array.isArray(value);
  const raw = isRecord(persisted) ? persisted : {};
  const normalized = { ...raw };

  // Read old local/saved-view snapshots once at the persistence boundary.
  // Shipping state uses executor exclusively; the legacy spelling must not
  // leak back into view contracts or consumers.
  const actorArray = (value: unknown): ActorFilterValue[] =>
    Array.isArray(value)
      ? value.filter(
          (actor): actor is ActorFilterValue =>
            isRecord(actor) &&
            (actor.type === "member" || actor.type === "agent" || actor.type === "team") &&
            typeof actor.id === "string",
        )
      : [];
  const legacyActors = actorArray(raw.assigneeFilters);
  if (normalized.executorFilters === undefined && legacyActors.length > 0) {
    normalized.executorFilters = legacyActors;
  }
  if (normalized.includeNoExecutor === undefined && raw.includeNoAssignee === true) {
    normalized.includeNoExecutor = true;
  }
  delete normalized.assigneeFilters;
  delete normalized.includeNoAssignee;

  for (const key of ["grouping", "swimlaneGrouping", "tableGrouping"] as const) {
    if (normalized[key] === "assignee") normalized[key] = "executor";
  }

  const migrateGroupingMap = (value: unknown): Record<string, unknown> | undefined => {
    if (!isRecord(value)) return undefined;
    const next = { ...value };
    if (next.executor === undefined && next.assignee !== undefined) {
      next.executor = next.assignee;
    }
    delete next.assignee;
    return next;
  };
  const swimlaneOrders = migrateGroupingMap(raw.swimlaneOrders);
  if (swimlaneOrders) normalized.swimlaneOrders = swimlaneOrders;
  const collapsedSwimlanes = migrateGroupingMap(raw.collapsedSwimlanes);
  if (collapsedSwimlanes) normalized.collapsedSwimlanes = collapsedSwimlanes;

  if (isRecord(raw.cardProperties)) {
    const cardProperties = { ...raw.cardProperties };
    if (
      cardProperties.executor === undefined &&
      typeof cardProperties.assignee === "boolean"
    ) {
      cardProperties.executor = cardProperties.assignee;
    }
    delete cardProperties.assignee;
    normalized.cardProperties = cardProperties;
  }

  if (Array.isArray(raw.tableColumns)) {
    normalized.tableColumns = raw.tableColumns.map((column) =>
      isRecord(column) && column.key === "assignee"
        ? { ...column, key: "executor" }
        : column,
    );
  }

  const p = normalized as Partial<T>;
  // `collapsedSwimlanes` changed shape from `string[]` to
  // `Record<SwimlaneGrouping, string[]>`. A snapshot saved in the old
  // shape would otherwise overwrite the default record with an array
  // and crash on first read — fall back to the default when the
  // persisted value isn't a plain object.
  const persistedTableColumns = Array.isArray(p.tableColumns)
    ? p.tableColumns.filter(
        (column): column is TableColumnConfig =>
          !!column &&
          typeof column === "object" &&
          typeof (column as TableColumnConfig).key === "string",
      )
    : current.tableColumns;
  const dedupedTableColumns = Array.from(
    new Map(persistedTableColumns.map((column) => [column.key, column])).values(),
  ).filter((column) => column.key !== "title");
  const persistedTitle = persistedTableColumns.find(
    (column) => column.key === "title",
  );
  return {
    ...current,
    ...p,
    cardProperties: {
      ...current.cardProperties,
      ...(p.cardProperties ?? {}),
    },
    swimlaneOrders: isRecord(p.swimlaneOrders)
      ? { ...current.swimlaneOrders, ...p.swimlaneOrders }
      : current.swimlaneOrders,
    collapsedSwimlanes: isRecord(p.collapsedSwimlanes)
      ? { ...current.collapsedSwimlanes, ...p.collapsedSwimlanes }
      : current.collapsedSwimlanes,
    tableColumns: [
      persistedTitle ?? current.tableColumns[0] ?? { key: "title" },
      ...dedupedTableColumns,
    ],
    tableCollapsedGroups: Array.isArray(p.tableCollapsedGroups)
      ? p.tableCollapsedGroups
      : current.tableCollapsedGroups,
    tableCollapsedParents: Array.isArray(p.tableCollapsedParents)
      ? p.tableCollapsedParents
      : current.tableCollapsedParents,
  };
}

/** Factory: creates a vanilla StoreApi for use with React Context. */
export function createIssueViewStore(persistKey: string): StoreApi<IssueViewState> {
  const store = createStore<IssueViewState>()(
    persist(viewStoreSlice, viewStorePersistOptions(persistKey))
  );
  registerForWorkspaceRehydration(() => store.persist.rehydrate());
  return store;
}

/** Global singleton for the /issues page. */
export const useIssueViewStore = create<IssueViewState>()(
  persist(viewStoreSlice, viewStorePersistOptions("patchbay_issues_view"))
);

registerForWorkspaceRehydration(() => useIssueViewStore.persist.rehydrate());

/**
 * Clears the given view store's filters whenever the workspace id changes.
 *
 * URL-driven: wsId arrives from `useWorkspaceId()` (Context fed by the
 * `[workspaceSlug]` route). We track the previous id via ref so the first
 * render doesn't wipe persisted filters — clearing only fires on transitions
 * from one defined workspace to another.
 */
export function useClearFiltersOnWorkspaceChange(
  store: StoreApi<IssueViewState> | { getState: () => IssueViewState },
  wsId: string | undefined,
) {
  const prevIdRef = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (prevIdRef.current && wsId && wsId !== prevIdRef.current) {
      store.getState().clearFilters();
    }
    prevIdRef.current = wsId;
  }, [wsId, store]);
}
