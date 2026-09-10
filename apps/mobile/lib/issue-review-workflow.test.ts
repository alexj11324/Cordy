import { describe, expect, it } from "vitest";
import {
  buildReviewSubmissionPatch,
  planIssueStatusSelection,
  reviewHandoffPatch,
  isReviewHandoff,
  reviewWorkflowViolation,
} from "./issue-review-workflow";

const executor = { type: "agent" as const, id: "agent-1" };
const reviewer = { type: "member" as const, id: "member-1" };

describe("issue review workflow", () => {
  it("requires an executor for every active status category", () => {
    for (const nextCategory of [
      "in_progress",
      "in_review",
      "blocked",
    ] as const) {
      expect(
        reviewWorkflowViolation({
          previousCategory: "todo",
          nextCategory,
          executor: null,
          reviewer,
        }),
      ).toBe("executor_required");
    }
  });

  it("requires a distinct reviewer only when entering review", () => {
    expect(
      reviewWorkflowViolation({
        previousCategory: "in_progress",
        nextCategory: "in_review",
        executor,
        reviewer: null,
      }),
    ).toBe("reviewer_required");

    expect(
      reviewWorkflowViolation({
        previousCategory: "in_progress",
        nextCategory: "in_review",
        executor,
        reviewer: executor,
      }),
    ).toBe("reviewer_must_differ");

    expect(
      reviewWorkflowViolation({
        previousCategory: "in_progress",
        nextCategory: "in_review",
        executor,
        reviewer,
      }),
    ).toBeNull();

    expect(
      reviewWorkflowViolation({
        previousCategory: "in_review",
        nextCategory: "in_review",
        executor,
        reviewer: null,
      }),
    ).toBeNull();
  });

  it("identifies a handoff by category instead of status key spelling", () => {
    expect(isReviewHandoff("in_progress", "in_review")).toBe(true);
    expect(isReviewHandoff("todo", "in_review")).toBe(true);
    expect(isReviewHandoff("in_review", "in_review")).toBe(false);
  });

  it("builds one atomic handoff patch without changing owner or executor", () => {
    const patch = reviewHandoffPatch("quality-review", reviewer);

    expect(patch).toEqual({
      status: "quality-review",
      reviewer_type: "member",
      reviewer_id: "member-1",
    });
    expect(patch).not.toHaveProperty("owner_type");
    expect(patch).not.toHaveProperty("executor_type");
  });

  it("routes status selection through the reviewer picker before entering review", () => {
    expect(
      planIssueStatusSelection({
        previousCategory: "in_progress",
        nextStatus: "quality-review",
        nextCategory: "in_review",
        executor,
        reviewer: null,
      }),
    ).toEqual({ kind: "choose_reviewer", status: "quality-review" });

    expect(
      planIssueStatusSelection({
        previousCategory: "in_progress",
        nextStatus: "quality-review",
        nextCategory: "in_review",
        executor,
        reviewer,
      }),
    ).toEqual({ kind: "collect_review_evidence", status: "quality-review" });
  });

  it("builds a complete atomic review handoff from trimmed evidence", () => {
    expect(
      buildReviewSubmissionPatch("quality-review", reviewer, {
        worktree: "  /worktrees/issue-42  ",
        branch: "  codex/issue-42  ",
        commit: `  ${"a".repeat(40)}  `,
        pullRequests:
          " https://github.com/example/repo/pull/42\n\nhttps://gitlab.com/example/repo/merge_requests/7 ",
      }),
    ).toEqual({
      status: "quality-review",
      reviewer_type: "member",
      reviewer_id: "member-1",
      review_submission: {
        worktree: "/worktrees/issue-42",
        branch: "codex/issue-42",
        commit: "a".repeat(40),
        pull_requests: [
          "https://github.com/example/repo/pull/42",
          "https://gitlab.com/example/repo/merge_requests/7",
        ],
      },
    });
  });

  it("rejects incomplete or malformed review evidence before mutation", () => {
    const valid = {
      worktree: "/worktrees/issue-42",
      branch: "codex/issue-42",
      commit: "a".repeat(40),
      pullRequests: "https://github.com/example/repo/pull/42",
    };

    expect(
      buildReviewSubmissionPatch("in_review", reviewer, {
        ...valid,
        commit: "abc123",
      }),
    ).toBeNull();
    expect(
      buildReviewSubmissionPatch("in_review", reviewer, {
        ...valid,
        pullRequests: "https://github.com/example/repo/issues/42",
      }),
    ).toBeNull();
    expect(
      buildReviewSubmissionPatch("in_review", reviewer, {
        ...valid,
        worktree: "   ",
      }),
    ).toBeNull();
  });

  it("blocks an active status selection until an executor exists", () => {
    expect(
      planIssueStatusSelection({
        previousCategory: "todo",
        nextStatus: "in_review",
        nextCategory: "in_review",
        executor: null,
        reviewer,
      }),
    ).toEqual({ kind: "blocked", violation: "executor_required" });
  });
});
