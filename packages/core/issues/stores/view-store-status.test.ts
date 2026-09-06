// @vitest-environment node
import { describe, expect, it, beforeEach } from "vitest";
import { createStore, type StoreApi } from "zustand/vanilla";
import {
  mergeViewStatePersisted,
  viewStoreSlice,
  type IssueViewState,
} from "./view-store";
import { baselineFromQuery } from "../../issue-views/baseline";

/**
 * Column visibility and the status filter used to be the same field. That was
 * only ever correct while a category held exactly one status; since MUL-6243 a
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

describe("card property defaults", () => {
  it("hides the description on board cards by default", () => {
    const store = createStore<IssueViewState>()((set) => viewStoreSlice(set));
    expect(store.getState().cardProperties.description).toBe(false);
  });
});

describe("legacy assignee view-state migration", () => {
  it("normalizes every persisted view dimension to executor", () => {
    const store = createStore<IssueViewState>()((set) => viewStoreSlice(set));
    const merged = mergeViewStatePersisted(
      {
        grouping: "assignee",
        assigneeFilters: [{ type: "agent", id: "agent-1" }],
        includeNoAssignee: true,
        cardProperties: { assignee: false },
        swimlaneGrouping: "assignee",
        swimlaneOrders: { assignee: ["agent:agent-1"] },
        collapsedSwimlanes: { assignee: ["agent:agent-2"] },
        tableColumns: [
          { key: "title", width: 320 },
          { key: "assignee", width: 180 },
        ],
        tableGrouping: "assignee",
      },
      store.getState(),
    );

    expect(merged.grouping).toBe("executor");
    expect(merged.executorFilters).toEqual([
      { type: "agent", id: "agent-1" },
    ]);
    expect(merged.includeNoExecutor).toBe(true);
    expect(merged.cardProperties.executor).toBe(false);
    expect(merged.swimlaneGrouping).toBe("executor");
    expect(merged.swimlaneOrders.executor).toEqual(["agent:agent-1"]);
    expect(merged.collapsedSwimlanes.executor).toEqual(["agent:agent-2"]);
    expect(merged.tableColumns.map((column) => column.key)).toEqual([
      "title",
      "executor",
    ]);
    expect(merged.tableGrouping).toBe("executor");
    expect(merged).not.toHaveProperty("assigneeFilters");
    expect(merged.cardProperties).not.toHaveProperty("assignee");
  });
});
