// @vitest-environment node
import { describe, expect, it, beforeEach } from "vitest";
import { createStore, type StoreApi } from "zustand/vanilla";
import { mergeViewStatePersisted, viewStoreSlice, type IssueViewState } from "./view-store";
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

describe("legacy view-state migration", () => {
  it("translates assignee fields before merging persisted state", () => {
    const current = createStore<IssueViewState>()((set) => viewStoreSlice(set)).getState();
    const merged = mergeViewStatePersisted(
      {
        grouping: "assignee",
        swimlaneGrouping: "assignee",
        tableGrouping: "assignee",
        assigneeFilters: [{ type: "member", id: "user-1" }],
        includeNoAssignee: true,
        cardProperties: { assignee: false },
        tableColumns: [{ key: "title" }, { key: "assignee" }],
        swimlaneOrders: { assignee: ["member:user-1"] },
        collapsedSwimlanes: { assignee: ["none"] },
      },
      current,
    );

    expect(merged.grouping).toBe("executor");
    expect(merged.swimlaneGrouping).toBe("executor");
    expect(merged.tableGrouping).toBe("executor");
    expect(merged.executorFilters).toEqual([{ type: "member", id: "user-1" }]);
    expect(merged.includeNoExecutor).toBe(true);
    expect(merged.cardProperties.executor).toBe(false);
    expect(merged.tableColumns.map((column) => column.key)).toEqual(["title", "executor"]);
    expect(merged.swimlaneOrders.executor).toEqual(["member:user-1"]);
    expect(merged.collapsedSwimlanes.executor).toEqual(["none"]);
  });
});
