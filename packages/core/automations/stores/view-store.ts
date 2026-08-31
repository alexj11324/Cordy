"use client";

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import {
  createWorkspaceAwareStorage,
  registerForWorkspaceRehydration,
} from "../../platform/workspace-storage";
import { defaultStorage } from "../../platform/storage";

// View preferences for the automations list page: scope, sort, column
// visibility, and filters. Persisted per workspace (workspace-aware storage),
// per user/device (localStorage). Search text and row selection are
// deliberately NOT stored — they are session-scoped (same rationale as the
// skills view store).

// Status is the promoted SCOPE dimension (lifecycle stage, mutually
// exclusive) — it therefore does NOT appear in `filters`; one dimension
// lives in exactly one place. "all" = active + paused. There is no
// archived scope because the product has no UI archiving flow (the DB
// status value exists but nothing in the UI can set it); add the scope
// back together with archive actions if that flow ever ships.
export type AutomationScope = "all" | "active" | "paused";

export const AUTOMATION_SCOPES: AutomationScope[] = ["all", "active", "paused"];

export type AutomationSortField = "name" | "lastRun" | "nextRun" | "created";

export type AutomationSortDirection = "asc" | "desc";

/** Per-field direction applied when the user switches TO that field. */
export const AUTOMATION_SORT_DEFAULT_DIRECTION: Record<
  AutomationSortField,
  AutomationSortDirection
> = {
  name: "asc",
  lastRun: "desc",
  nextRun: "asc",
  created: "desc",
};

/** Multi-select filter state. Empty array per dimension = inactive. */
export interface AutomationListFilters {
  executors: string[];
  modes: string[];
  triggerKinds: string[];
  creators: string[];
}

export const EMPTY_AUTOMATION_FILTERS: AutomationListFilters = {
  executors: [],
  modes: [],
  triggerKinds: [],
  creators: [],
};

// User-hideable columns. Name and the structural columns (checkbox, kebab)
// are always visible.
export type AutomationColumnKey =
  | "executor"
  | "trigger"
  | "lastRun"
  | "nextRun"
  | "mode"
  | "creator"
  | "created";

/** Mode, creator and created are opt-in: hidden until the user enables them. */
export const AUTOMATION_DEFAULT_HIDDEN_COLUMNS: AutomationColumnKey[] = [
  "mode",
  "creator",
  "created",
];

export interface AutomationsViewState {
  scope: AutomationScope;
  sortField: AutomationSortField;
  sortDirection: AutomationSortDirection;
  hiddenColumns: AutomationColumnKey[];
  filters: AutomationListFilters;
  setScope: (scope: AutomationScope) => void;
  /** Header click: toggles direction on the active field, otherwise switches
   *  to the field with its default direction. */
  toggleSort: (field: AutomationSortField) => void;
  /** Display panel select: switches field (default direction), no toggle. */
  setSortField: (field: AutomationSortField) => void;
  setSortDirection: (direction: AutomationSortDirection) => void;
  toggleColumn: (key: AutomationColumnKey) => void;
  toggleFilter: (key: keyof AutomationListFilters, value: string) => void;
  clearFilters: () => void;
}

const DEFAULTS = {
  scope: "all" as AutomationScope,
  sortField: "lastRun" as AutomationSortField,
  sortDirection: AUTOMATION_SORT_DEFAULT_DIRECTION.lastRun,
  hiddenColumns: AUTOMATION_DEFAULT_HIDDEN_COLUMNS,
  filters: EMPTY_AUTOMATION_FILTERS,
};

export const useAutomationsViewStore = create<AutomationsViewState>()(
  persist(
    (set) => ({
      ...DEFAULTS,
      setScope: (scope) => set({ scope }),
      toggleSort: (field) =>
        set((state) =>
          state.sortField === field
            ? {
                sortDirection: state.sortDirection === "asc" ? "desc" : "asc",
              }
            : {
                sortField: field,
                sortDirection: AUTOMATION_SORT_DEFAULT_DIRECTION[field],
              },
        ),
      setSortField: (field) =>
        set((state) =>
          state.sortField === field
            ? {}
            : {
                sortField: field,
                sortDirection: AUTOMATION_SORT_DEFAULT_DIRECTION[field],
              },
        ),
      setSortDirection: (direction) => set({ sortDirection: direction }),
      toggleColumn: (key) =>
        set((state) => ({
          hiddenColumns: state.hiddenColumns.includes(key)
            ? state.hiddenColumns.filter((k) => k !== key)
            : [...state.hiddenColumns, key],
        })),
      toggleFilter: (key, value) =>
        set((state) => {
          const list = state.filters[key] as string[];
          const next = list.includes(value)
            ? list.filter((v) => v !== value)
            : [...list, value];
          return { filters: { ...state.filters, [key]: next } };
        }),
      clearFilters: () => set({ filters: EMPTY_AUTOMATION_FILTERS }),
    }),
    {
      name: "patchbay_automations_view",
      storage: createJSONStorage(() =>
        createWorkspaceAwareStorage(defaultStorage),
      ),
      partialize: (state) => ({
        scope: state.scope,
        sortField: state.sortField,
        sortDirection: state.sortDirection,
        hiddenColumns: state.hiddenColumns,
        filters: state.filters,
      }),
      // On rehydrate, if the new workspace has no persisted value, reset to
      // the defaults instead of leaving the previous workspace's in-memory
      // view state in place (same rationale as the skills view store).
      merge: (persisted, current) => {
        if (!persisted) return { ...current, ...DEFAULTS };
        const p = persisted as Partial<AutomationsViewState>;
        // Deep-merge filters so a payload persisted before a new filter
        // dimension existed still gets that key's default instead of
        // dropping it to undefined (which crashes `.length` reads).
        return {
          ...current,
          ...p,
          filters: { ...EMPTY_AUTOMATION_FILTERS, ...(p.filters ?? {}) },
        };
      },
    },
  ),
);

registerForWorkspaceRehydration(() =>
  useAutomationsViewStore.persist.rehydrate(),
);
