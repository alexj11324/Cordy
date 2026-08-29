"use client";

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import {
  createWorkspaceAwareStorage,
  registerForWorkspaceRehydration,
} from "../../platform/workspace-storage";
import { defaultStorage } from "../../platform/storage";

// View preferences for the teams list page: scope, sort, column visibility.
// Persisted per workspace, per user/device. No filters (the set is tiny);
// no search (scope-bearing list). Mirrors the agents/skills view stores.

// Scope is the ownership lens (creator-based). No "archived" scope: the
// list endpoint hard-filters archived teams and there is no restore
// endpoint, so archived teams can't be surfaced or managed.
export type TeamsScope = "mine" | "all";

export const TEAM_SCOPES: TeamsScope[] = ["mine", "all"];

export type TeamSortField = "name" | "members" | "created";

export type TeamSortDirection = "asc" | "desc";

/** Per-field direction applied when the user switches TO that field. */
export const TEAM_SORT_DEFAULT_DIRECTION: Record<
  TeamSortField,
  TeamSortDirection
> = {
  name: "asc",
  members: "desc",
  created: "desc",
};

// User-hideable columns. Name and leader (the team's defining relationship)
// are always visible.
export type TeamColumnKey = "members" | "creator" | "created";

/** Created (date) is opt-in. Creator ("Created by") is shown by default —
 *  the user wants to see who made each team. Note it's "Created by", NOT
 *  "Owner": the team creator holds no management rights (archiving is
 *  workspace-admin only), so labelling it Owner would mislead. */
export const TEAM_DEFAULT_HIDDEN_COLUMNS: TeamColumnKey[] = ["created"];

/** Multi-select filters — the categorical columns (leader, creator). Empty
 *  array per dimension = inactive. */
export interface TeamListFilters {
  /** Leader agent ids. */
  leaders: string[];
  /** Creator member user ids. */
  creators: string[];
}

export const EMPTY_TEAM_FILTERS: TeamListFilters = {
  leaders: [],
  creators: [],
};

export interface TeamsViewState {
  scope: TeamsScope;
  sortField: TeamSortField;
  sortDirection: TeamSortDirection;
  hiddenColumns: TeamColumnKey[];
  filters: TeamListFilters;
  setScope: (scope: TeamsScope) => void;
  /** Header click: toggles direction on the active field, otherwise switches
   *  to the field with its default direction. */
  toggleSort: (field: TeamSortField) => void;
  /** Display panel select: switches field (default direction), no toggle. */
  setSortField: (field: TeamSortField) => void;
  setSortDirection: (direction: TeamSortDirection) => void;
  toggleColumn: (key: TeamColumnKey) => void;
  toggleFilter: (key: keyof TeamListFilters, value: string) => void;
  clearFilters: () => void;
}

const DEFAULTS = {
  scope: "mine" as TeamsScope,
  sortField: "name" as TeamSortField,
  sortDirection: TEAM_SORT_DEFAULT_DIRECTION.name,
  hiddenColumns: TEAM_DEFAULT_HIDDEN_COLUMNS,
  filters: EMPTY_TEAM_FILTERS,
};

export const useTeamsViewStore = create<TeamsViewState>()(
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
                sortDirection: TEAM_SORT_DEFAULT_DIRECTION[field],
              },
        ),
      setSortField: (field) =>
        set((state) =>
          state.sortField === field
            ? {}
            : {
                sortField: field,
                sortDirection: TEAM_SORT_DEFAULT_DIRECTION[field],
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
      clearFilters: () => set({ filters: EMPTY_TEAM_FILTERS }),
    }),
    {
      name: "patchbay_teams_view",
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
      // the defaults instead of leaking the previous workspace's state.
      // Deep-merge filters so a pre-filters payload backfills defaults.
      merge: (persisted, current) => {
        if (!persisted) return { ...current, ...DEFAULTS };
        const p = persisted as Partial<TeamsViewState>;
        return {
          ...current,
          ...p,
          filters: { ...EMPTY_TEAM_FILTERS, ...(p.filters ?? {}) },
        };
      },
    },
  ),
);

registerForWorkspaceRehydration(() => useTeamsViewStore.persist.rehydrate());
