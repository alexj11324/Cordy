import { describe, expect, it } from "vitest";
import {
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
});
