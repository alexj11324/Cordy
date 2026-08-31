// @vitest-environment node
import { describe, expect, it, beforeEach } from "vitest";
import { createStore, type StoreApi } from "zustand/vanilla";
import {
  migrateLegacyViewState,
  viewStoreSlice,
  type IssueViewState,
} from "./view-store";
import { baselineFromQuery } from "../../issue-views/baseline";

/**
 * Column visibility and the status filter used to be the same field. That was
 * only ever correct while a category held exactly one status; since PB-6243 a
 * category can hold several, and the two questions have different answers.
 */
describe("column visibility vs status filter", () => {
  let store: StoreApi<IssueViewState>;
  beforeEach(() => {
    store = createStore<IssueViewState>()((set) => viewStoreSlice(set));
  });

  // The regression: hiding one column wrote the OTHER six built-in keys into
  // statusFilters, so the query then excluded every custom status too — hiding
  // Backlog silently dropped a QA card sitting in the In Review column.
  it("hiding a column does not touch the status filter", () => {
    store.getState().hideStatus("backlog");

    expect(store.getState().hiddenStatusCategories).toEqual(["backlog"]);
    expect(store.getState().statusFilters).toEqual([]);
  });

  it("showing a column restores it without inventing a filter", () => {
    store.getState().hideStatus("backlog");
    store.getState().hideStatus("done");
    store.getState().showStatus("backlog");

    expect(store.getState().hiddenStatusCategories).toEqual(["done"]);
    expect(store.getState().statusFilters).toEqual([]);
  });

  it("hiding the same column twice is idempotent", () => {
    store.getState().hideStatus("backlog");
    store.getState().hideStatus("backlog");

    expect(store.getState().hiddenStatusCategories).toEqual(["backlog"]);
  });

  it("a custom status filter survives hiding and showing a column", () => {
    store.getState().toggleStatusFilter("qa");
    store.getState().hideStatus("backlog");
    store.getState().showStatus("backlog");

    expect(store.getState().statusFilters).toEqual(["qa"]);
  });

  it("reset restores every column", () => {
    store.getState().hideStatus("backlog");
    store.getState().clearFilters();

    expect(store.getState().hiddenStatusCategories).toEqual([]);
  });
});

describe("saved view baseline", () => {
  // The regression: the baseline dropped any status filter that was not one of
  // the 7 built-ins, so reopening a view saved with a custom status filter came
  // back showing MORE than it was saved with.
  it("keeps a custom status filter", () => {
    const baseline = baselineFromQuery({ statusFilters: ["in_review", "qa"] });

    expect(baseline.status.has("qa")).toBe(true);
    expect(baseline.status.has("in_review")).toBe(true);
  });

  it("still drops values it cannot represent", () => {
    const baseline = baselineFromQuery({ statusFilters: ["", "qa"] });

    expect([...baseline.status]).toEqual(["qa"]);
  });
});

describe("legacy assignee view-state migration", () => {
  it("normalizes persisted filters, groupings, columns, and card properties", () => {
    const migrated = migrateLegacyViewState({
      assigneeFilters: [{ type: "member", id: "member-1" }],
      includeNoAssignee: true,
      grouping: "assignee",
      tableGrouping: "assignee",
      swimlaneGrouping: "assignee",
      tableColumns: [{ key: "assignee", width: 160 }],
      cardProperties: { assignee: false, priority: true },
    });

    expect(migrated).toMatchObject({
      executorFilters: [{ type: "member", id: "member-1" }],
      includeNoExecutor: true,
      grouping: "executor",
      tableGrouping: "executor",
      swimlaneGrouping: "executor",
      tableColumns: [{ key: "executor", width: 160 }],
      cardProperties: { executor: false, priority: true },
    });
    expect(migrated).not.toHaveProperty("assigneeFilters");
    expect(migrated).not.toHaveProperty("includeNoAssignee");
    expect(migrated.cardProperties).not.toHaveProperty("assignee");
  });

  it("does not overwrite canonical values with legacy fields", () => {
    const migrated = migrateLegacyViewState({
      executorFilters: [{ type: "agent", id: "agent-1" }],
      assigneeFilters: [{ type: "member", id: "member-1" }],
      includeNoExecutor: false,
      includeNoAssignee: true,
    });

    expect(migrated.executorFilters).toEqual([{ type: "agent", id: "agent-1" }]);
    expect(migrated.includeNoExecutor).toBe(false);
  });
});
