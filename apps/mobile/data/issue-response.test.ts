import { describe, expect, it } from "vitest";
import type { Issue } from "@patchbay/core/types";
import {
  parseCreatedIssueResponse,
  parseUpdatedIssueResponse,
} from "./issue-response";

function issue(overrides: Partial<Issue> = {}): Issue {
  return {
    id: "issue-1",
    workspace_id: "workspace-1",
    number: 1,
    identifier: "COR-1",
    title: "Review mobile",
    description: null,
    status: "in_review",
    status_category: "in_review",
    priority: "medium",
    owner_type: "member",
    owner_id: "member-1",
    executor_type: "agent",
    executor_id: "agent-1",
    reviewer_type: "member",
    reviewer_id: "member-2",
    creator_type: "member",
    creator_id: "member-1",
    parent_issue_id: null,
    project_id: null,
    position: 0,
    stage: null,
    start_date: null,
    due_date: null,
    metadata: {},
    properties: {},
    created_at: "2026-09-03T00:00:00Z",
    updated_at: "2026-09-03T00:00:00Z",
    ...overrides,
  };
}

describe("issue response boundary", () => {
  it("preserves all three independent roles through create parsing", () => {
    expect(parseCreatedIssueResponse(issue())).toMatchObject({
      owner_id: "member-1",
      executor_id: "agent-1",
      reviewer_id: "member-2",
    });
  });

  it("rejects a malformed create response instead of reporting success", () => {
    expect(() =>
      parseCreatedIssueResponse({ title: "missing id" }),
    ).toThrow("Invalid issue create response");
  });

  it("rejects an update response for a different issue", () => {
    expect(() =>
      parseUpdatedIssueResponse("issue-1", issue({ id: "issue-2" })),
    ).toThrow("Invalid issue update response");
  });

  it("keeps a future reviewer type observable after schema parsing", () => {
    const raw: unknown = {
      ...issue(),
      reviewer_type: "external_reviewer",
    };

    const parsed = parseUpdatedIssueResponse("issue-1", raw);
    expect(
      (parsed as unknown as { reviewer_type: string }).reviewer_type,
    ).toBe("external_reviewer");
  });
});
