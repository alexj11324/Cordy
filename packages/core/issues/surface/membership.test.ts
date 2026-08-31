import { describe, expect, it } from "vitest";
import type { Issue } from "../../types";
import {
  issueChangedDims,
  issueMatchesListFilter,
  listFilterDependsOn,
} from "./membership";

function makeIssue(overrides: Partial<Issue> = {}): Issue {
  return {
    id: "issue-1",
    workspace_id: "ws-1",
    number: 1,
    identifier: "PB-1",
    title: "Issue 1",
    description: null,
    status: "todo",
    priority: "none",
    owner_type: "member",
    owner_id: "me",
    executor_type: null,
    executor_id: null,
    reviewer_type: null,
    reviewer_id: null,
    creator_type: "member",
    creator_id: "me",
    parent_issue_id: null,
    project_id: "p1",
    position: 1,
    stage: null,
    start_date: null,
    due_date: null,
    labels: [],
    metadata: {},
  properties: {},
    created_at: "2025-01-01T00:00:00Z",
    updated_at: "2025-01-01T00:00:00Z",
    ...overrides,
  };
}

describe("issueMatchesListFilter", () => {
  it("judges owner_id filters definitively", () => {
    const issue = makeIssue();
    expect(issueMatchesListFilter(issue, "assigned", { owner_id: "me" })).toBe(true);
    expect(issueMatchesListFilter(issue, "assigned", { owner_id: "bob" })).toBe(false);
  });

  it("degrades to unknown when the entity is missing the filtered field", () => {
    expect(
      issueMatchesListFilter({ title: "partial" }, "assigned", { owner_id: "me" }),
    ).toBe("unknown");
  });

  it("judges owner and executor type filters independently", () => {
    expect(
      issueMatchesListFilter(makeIssue(), "workspace:members", {
        owner_types: ["member"],
      }),
    ).toBe(true);
    expect(
      issueMatchesListFilter(makeIssue({ owner_type: null, owner_id: null }), "workspace:members", {
        owner_types: ["member"],
      }),
    ).toBe(false);
    expect(
      issueMatchesListFilter(
        makeIssue({ executor_type: null, executor_id: null }),
        "workspace:agents",
        { executor_types: ["agent", "team"] },
      ),
    ).toBe(false);
  });

  it("judges project filters", () => {
    expect(
      issueMatchesListFilter(makeIssue(), "project:p1", { project_id: "p1" }),
    ).toBe(true);
    expect(
      issueMatchesListFilter(makeIssue({ project_id: "p2" }), "project:p1", {
        project_id: "p1",
      }),
    ).toBe(false);
    expect(
      issueMatchesListFilter(makeIssue({ project_id: null }), "project:p1", {
        project_id: "p1",
      }),
    ).toBe(false);
  });

  it("never decides involves_user_id — the ownership graph is server-side", () => {
    expect(
      issueMatchesListFilter(makeIssue(), "agents", { involves_user_id: "me" }),
    ).toBe("unknown");
  });

  it("never decides the my:all union scope", () => {
    expect(issueMatchesListFilter(makeIssue(), "all", {})).toBe("unknown");
  });

  it("ANDs across fields — a definitive miss beats an unknown", () => {
    expect(
      issueMatchesListFilter(
        makeIssue({ project_id: "p2" }),
        "scoped",
        { project_id: "p1", involves_user_id: "me" },
      ),
    ).toBe(false);
  });
});

describe("issueChangedDims", () => {
  it("treats written membership fields as changed when no base is known", () => {
    expect(issueChangedDims({ owner_id: "bob", owner_type: "member" })).toEqual({
      owner: true,
      executor: false,
      project: false,
      status: false,
    });
    expect(issueChangedDims({ project_id: null })).toEqual({
      owner: false,
      executor: false,
      project: true,
      status: false,
    });
  });

  it("sharpens against a base entity — writing the same value changes nothing", () => {
    const base = makeIssue();
    expect(issueChangedDims({ owner_id: "me", owner_type: "member" }, base)).toEqual({
      owner: false,
      executor: false,
      project: false,
      status: false,
    });
    expect(issueChangedDims({ status: "todo" }, base).status).toBe(false);
    expect(issueChangedDims({ status: "done" }, base).status).toBe(true);
    expect(issueChangedDims({ project_id: "p2" }, base).project).toBe(true);
  });

  it("ignores non-membership fields", () => {
    expect(issueChangedDims({ title: "x", position: 9 })).toEqual({
      owner: false,
      executor: false,
      project: false,
      status: false,
    });
  });
});

describe("listFilterDependsOn", () => {
  const none = { owner: false, executor: false, project: false, status: false };

  it("my:all reacts to owner and executor changes", () => {
    expect(listFilterDependsOn("all", {}, { ...none, owner: true })).toBe(true);
    expect(listFilterDependsOn("all", {}, { ...none, executor: true })).toBe(true);
    expect(listFilterDependsOn("all", {}, { ...none, project: true })).toBe(false);
  });

  it("role-keyed filters react to their own role changes", () => {
    expect(
      listFilterDependsOn("assigned", { owner_id: "me" }, { ...none, owner: true }),
    ).toBe(true);
    expect(
      listFilterDependsOn(
        "workspace:members",
        { owner_types: ["member"] },
        { ...none, owner: true },
      ),
    ).toBe(true);
    expect(
      listFilterDependsOn("agents", { involves_user_id: "me" }, { ...none, executor: true }),
    ).toBe(true);
    expect(
      listFilterDependsOn("assigned", { owner_id: "me" }, { ...none, project: true }),
    ).toBe(false);
  });

  it("project filters react to project changes", () => {
    expect(
      listFilterDependsOn("project:p1", { project_id: "p1" }, { ...none, project: true }),
    ).toBe(true);
    expect(
      listFilterDependsOn("project:p1", { project_id: "p1" }, { ...none, executor: true }),
    ).toBe(false);
  });

  it("creator filters never react — creator is immutable", () => {
    expect(
      listFilterDependsOn(
        "created",
        { creator_id: "me" },
        { owner: true, executor: true, project: true, status: true },
      ),
    ).toBe(false);
  });

  it("the unfiltered workspace list never reacts", () => {
    expect(
      listFilterDependsOn(undefined, {}, { owner: true, executor: true, project: true, status: true }),
    ).toBe(false);
  });
});
