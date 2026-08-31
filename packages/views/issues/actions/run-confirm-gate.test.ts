// @vitest-environment node
import { describe, it, expect } from "vitest";
import { buildIssueStatusCatalog } from "@patchbay/core/issue-statuses";
import type { IssueStatusEntry } from "@patchbay/core/types";
import { runConfirmIntent, resolveStatusCategory, type GateIssue } from "./run-confirm-gate";

// The canonical matrix for "does this write need confirming" (PB-6463). The
// hook and table suites only prove they route on this answer.
function entry(key: string, category: string): IssueStatusEntry {
  return {
    id: key,
    workspace_id: "ws-1",
    key,
    name: key,
    description: "",
    category: category as IssueStatusEntry["category"],
    color: "#ff0000",
    is_system: false,
    position: 0,
    archived_at: null,
    created_at: "",
    updated_at: "",
  };
}

// Custom statuses cover each category whose scheduling semantics matter.
const CATALOG = buildIssueStatusCatalog([
  entry("later", "backlog"),
  entry("rework", "todo"),
  entry("doing", "in_progress"),
  entry("qa", "in_review"),
]);
// A catalog that has not loaded: every custom key is unresolvable.
const COLD = buildIssueStatusCatalog(undefined);

function issue(overrides: Partial<GateIssue> = {}): GateIssue {
  return {
    id: "issue-1",
    revision: 7,
    status: "backlog",
    executor_type: "agent",
    executor_id: "agent-1",
    reviewer_type: null,
    reviewer_id: null,
    ...overrides,
  };
}

describe("resolveStatusCategory", () => {
  it("prefers the category the payload carries", () => {
    expect(resolveStatusCategory("qa", "in_review", COLD)).toBe("in_review");
  });

  it("resolves a built-in key without a catalog", () => {
    expect(resolveStatusCategory("backlog", undefined, COLD)).toBe("backlog");
  });

  it("resolves a custom key through the catalog", () => {
    expect(resolveStatusCategory("later", undefined, CATALOG)).toBe("backlog");
  });

  it("answers null — never a guess — for a custom key nothing can resolve", () => {
    // catalog.categoryOf would say `todo` here, which is the guess this gate
    // exists to avoid: it decides whether a write may start an agent.
    expect(CATALOG.categoryOf("unknown")).toBe("todo");
    expect(resolveStatusCategory("unknown", undefined, CATALOG)).toBeNull();
    expect(resolveStatusCategory("later", undefined, COLD)).toBeNull();
  });
});

describe("runConfirmIntent — assign", () => {
  it("confirms when changing the executor of an In Progress issue", () => {
    expect(
      runConfirmIntent(
        issue({ status: "in_progress" }),
        { executor_type: "agent", executor_id: "a-2" },
        CATALOG,
      ),
    ).toEqual({ issueIds: ["issue-1"], mode: "assign", executorType: "agent", executorId: "a-2" });
  });

  it.each([
    ["Backlog", "backlog", undefined],
    ["Todo", "todo", undefined],
    ["Blocked", "blocked", undefined],
    ["In Review", "in_review", undefined],
    ["Done", "done", undefined],
    ["custom Todo", "rework", undefined],
    ["category carried by the payload", "anything", "todo" as const],
  ])("applies directly for %s because assigning there does not start execution", (_label, status, carried) => {
    expect(
      runConfirmIntent(
        issue({ status, status_category: carried }),
        { executor_type: "agent", executor_id: "a-2" },
        CATALOG,
      ),
    ).toBeNull();
  });

  it("confirms when the issue's own category is unresolvable", () => {
    // Fails toward the dialog: a dismissed dialog costs a click, a silent
    // start costs an agent run.
    expect(
      runConfirmIntent(issue({ status: "later" }), { executor_type: "agent", executor_id: "a-2" }, COLD),
    ).not.toBeNull();
  });

  it("applies directly when assigning a human owner", () => {
    expect(
      runConfirmIntent(issue({ status: "todo" }), { owner_type: "member", owner_id: "u-1" }, CATALOG),
    ).toBeNull();
  });
});

describe("runConfirmIntent — promote", () => {
  it.each([
    ["Backlog", "backlog", "in_progress"],
    ["Todo", "todo", "in_progress"],
    ["Blocked", "blocked", "in_progress"],
    ["custom Todo origin", "rework", "in_progress"],
    ["custom In Progress target", "todo", "doing"],
  ])("confirms the promotion (%s)", (_label, from, to) => {
    expect(runConfirmIntent(issue({ status: from }), { status: to }, CATALOG)).toEqual({
      issueIds: ["issue-1"],
      mode: "promote",
      status: to,
      executorType: "agent",
      executorId: "agent-1",
    });
  });

  it("confirms when the target is unresolvable and may start a run", () => {
    expect(
      runConfirmIntent(issue({ status: "backlog" }), { status: "unknown" }, CATALOG),
    ).not.toBeNull();
  });

  it.each([
    ["no executor", { status: "todo", executor_type: null, executor_id: null }, "in_progress"],
    ["Backlog to Todo", { status: "backlog" }, "todo"],
    ["Backlog to custom Todo", { status: "backlog" }, "rework"],
    ["already In Progress", { status: "in_progress" }, "doing"],
    ["closing the issue", { status: "backlog" }, "done"],
    ["cancelling the issue", { status: "backlog" }, "cancelled"],
    ["re-parking inside backlog", { status: "backlog" }, "later"],
    ["the same status again", { status: "backlog" }, "backlog"],
  ])("applies directly when no run starts (%s)", (_label, from, to) => {
    expect(runConfirmIntent(issue(from as Partial<GateIssue>), { status: to }, CATALOG)).toBeNull();
  });
});

describe("runConfirmIntent — review handoff", () => {
  it.each([
    ["built-in review", "in_review"],
    ["custom review", "qa"],
  ])("requires choosing a different reviewer for %s", (_label, status) => {
    expect(
      runConfirmIntent(issue({ status: "in_progress" }), { status }, CATALOG),
    ).toEqual({
      issueIds: ["issue-1"],
      mode: "review",
      status,
      fromExecutorType: "agent",
      fromExecutorId: "agent-1",
      executorType: null,
      executorId: null,
      issueRevision: 7,
    });
  });

  it("keeps an explicitly selected reviewer in the atomic transition", () => {
    expect(
      runConfirmIntent(
        issue({ status: "in_progress" }),
        { status: "in_review", reviewer_type: "member", reviewer_id: "user-2" },
        CATALOG,
      ),
    ).toBeNull();
  });

  it("applies directly when the issue already has a reviewer", () => {
    expect(
      runConfirmIntent(
        issue({
          status: "in_progress",
          reviewer_type: "member",
          reviewer_id: "user-2",
        }),
        { status: "in_review" },
        CATALOG,
      ),
    ).toBeNull();
  });

  it("does not ask for another handoff within the review category", () => {
    expect(
      runConfirmIntent(
        issue({ status: "qa", status_category: "in_review" }),
        { status: "in_review" },
        CATALOG,
      ),
    ).toBeNull();
  });
});

describe("runConfirmIntent — review return", () => {
  it.each([
    ["built-in review", "in_review"],
    ["custom review", "qa"],
  ])("confirms returning the implementation owner from %s", (_label, status) => {
    expect(
      runConfirmIntent(issue({ status }), { status: "in_progress" }, CATALOG),
    ).toEqual({
      issueIds: ["issue-1"],
      mode: "review-return",
      status: "in_progress",
      executorType: "agent",
      executorId: "agent-1",
      issueRevision: 7,
    });
  });

  it("confirms when a grouped surface repeats the current owner fields", () => {
    expect(
      runConfirmIntent(
        issue({ status: "in_review" }),
        {
          status: "in_progress",
          executor_type: "agent",
          executor_id: "agent-1",
        },
        CATALOG,
      ),
    ).toMatchObject({ mode: "review-return" });
  });

  it.each([
    ["no executor", { executor_type: null, executor_id: null }],
  ])("does not offer an agent handoff for a %s", (_label, owner) => {
    expect(
      runConfirmIntent(issue({ status: "in_review", ...owner }), { status: "in_progress" }, CATALOG),
    ).toBeNull();
  });
});
