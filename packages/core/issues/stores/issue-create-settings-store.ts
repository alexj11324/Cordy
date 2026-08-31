"use client";

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import { createWorkspaceAwareStorage, registerForWorkspaceRehydration } from "../../platform/workspace-storage";
import { defaultStorage } from "../../platform/storage";

export type QuickCreateField = "project" | "priority" | "due_date";
export type ManualCreateField =
  | "status"
  | "priority"
  | "owner"
  | "executor"
  | "reviewer"
  | "labels"
  | "project"
  | "due_date"
  | "start_date";

// Canonical field order — the settings tab renders rows in this order and
// setters normalize persisted arrays against it, so a toggle sequence never
// produces two different persisted encodings of the same selection.
export const QUICK_CREATE_FIELDS: QuickCreateField[] = ["project", "priority", "due_date"];
export const MANUAL_CREATE_FIELDS: ManualCreateField[] = [
  "status",
  "priority",
  "owner",
  "executor",
  "reviewer",
  "labels",
  "project",
  "due_date",
  "start_date",
];

export const DEFAULT_QUICK_CREATE_FIELDS: QuickCreateField[] = ["project"];
// Keep the first-run form focused on execution. Owner is still defaulted to
// the current member by the create dialog, but its picker is available from
// the overflow. Reviewer appears when a review status or workspace policy
// requires it.
export const DEFAULT_MANUAL_CREATE_FIELDS: ManualCreateField[] = [
  "status",
  "priority",
  "executor",
  "labels",
  "project",
];

// Which optional fields each create-issue mode keeps on its toolbar. Owned by
// Settings → Issue and read by both create dialogs; a field toggled off here
// stays reachable from the dialog's ⋯ overflow and always re-surfaces while it
// holds a value. Per-workspace via the workspace-aware storage (projects and
// custom properties differ per workspace), per-user for free from
// localStorage being browser-profile-local — same scoping as quick-create's
// actor/project memory.
interface IssueCreateSettingsState {
  quickCreateFields: QuickCreateField[];
  setQuickCreateFieldVisible: (field: QuickCreateField, visible: boolean) => void;
  manualCreateFields: ManualCreateField[];
  setManualCreateFieldVisible: (field: ManualCreateField, visible: boolean) => void;
  resetToDefaults: () => void;
}

function toggle<F extends string>(all: F[], current: F[], field: F, visible: boolean): F[] {
  return all.filter((f) => (f === field ? visible : current.includes(f)));
}

export function normalizeIssueCreateFields<F extends string>(
  value: unknown,
  all: readonly F[],
  fallback: readonly F[],
): F[] {
  if (!Array.isArray(value)) return [...fallback];
  const known = new Set(
    value.filter(
      (field): field is F =>
        typeof field === "string" && (all as readonly string[]).includes(field),
    ),
  );
  if (value.length > 0 && known.size === 0) return [...fallback];
  return all.filter((field) => known.has(field));
}

type PersistedIssueCreateSettings = {
  quickCreateFields?: unknown;
  manualCreateFields?: unknown;
};

export function migrateIssueCreateSettings(
  persistedState: unknown,
  version: number,
): Pick<IssueCreateSettingsState, "quickCreateFields" | "manualCreateFields"> {
  const persisted =
    persistedState && typeof persistedState === "object"
      ? (persistedState as PersistedIssueCreateSettings)
      : {};
  let manualFields = persisted.manualCreateFields;
  if (version < 2 && Array.isArray(manualFields) && manualFields.includes("assignee")) {
    // The old single assignee picker represented both human responsibility
    // and execution. Preserve that user's intent in the two explicit role
    // pickers; otherwise the new default keeps Owner out of the first-run
    // toolbar while still defaulting it to the current member.
    manualFields = [
      ...manualFields.filter((field) => field !== "assignee"),
      "owner",
      "executor",
    ];
  }
  return {
    quickCreateFields: normalizeIssueCreateFields(
      persisted.quickCreateFields,
      QUICK_CREATE_FIELDS,
      DEFAULT_QUICK_CREATE_FIELDS,
    ),
    manualCreateFields: normalizeIssueCreateFields(
      manualFields,
      MANUAL_CREATE_FIELDS,
      DEFAULT_MANUAL_CREATE_FIELDS,
    ),
  };
}

export const useIssueCreateSettingsStore = create<IssueCreateSettingsState>()(
  persist(
    (set) => ({
      quickCreateFields: DEFAULT_QUICK_CREATE_FIELDS,
      setQuickCreateFieldVisible: (field, visible) =>
        set((s) => ({
          quickCreateFields: toggle(QUICK_CREATE_FIELDS, s.quickCreateFields, field, visible),
        })),
      manualCreateFields: DEFAULT_MANUAL_CREATE_FIELDS,
      setManualCreateFieldVisible: (field, visible) =>
        set((s) => ({
          manualCreateFields: toggle(MANUAL_CREATE_FIELDS, s.manualCreateFields, field, visible),
        })),
      resetToDefaults: () =>
        set({
          quickCreateFields: [...DEFAULT_QUICK_CREATE_FIELDS],
          manualCreateFields: [...DEFAULT_MANUAL_CREATE_FIELDS],
        }),
    }),
    {
      name: "patchbay_issue_create_settings",
      storage: createJSONStorage(() => createWorkspaceAwareStorage(defaultStorage)),
      version: 2,
      migrate: (persistedState, version) =>
        migrateIssueCreateSettings(persistedState, version),
      merge: (persistedState, currentState) => {
        const normalized = migrateIssueCreateSettings(persistedState, 2);
        return {
          ...currentState,
          ...normalized,
        };
      },
    },
  ),
);

registerForWorkspaceRehydration(() => useIssueCreateSettingsStore.persist.rehydrate());
