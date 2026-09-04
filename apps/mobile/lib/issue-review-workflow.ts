import type {
  IssueActorType,
  IssueStatus,
  IssueStatusCategory,
} from "@patchbay/core/types";

export type IssueRoleRef = { type: IssueActorType; id: string };

export type ReviewHandoffPatch = {
  status: IssueStatus;
  reviewer_type: IssueActorType;
  reviewer_id: string;
};

export type ReviewWorkflowViolation =
  | "executor_required"
  | "reviewer_required"
  | "reviewer_must_differ";

const EXECUTOR_REQUIRED_CATEGORIES: readonly IssueStatusCategory[] = [
  "in_progress",
  "in_review",
  "blocked",
];

export function isReviewHandoff(
  previousCategory: IssueStatusCategory | null | undefined,
  nextCategory: IssueStatusCategory,
): boolean {
  return previousCategory !== "in_review" && nextCategory === "in_review";
}

export function reviewHandoffPatch(
  status: IssueStatus,
  reviewer: IssueRoleRef,
): ReviewHandoffPatch {
  return {
    status,
    reviewer_type: reviewer.type,
    reviewer_id: reviewer.id,
  };
}

function sameActor(
  left: IssueRoleRef | null,
  right: IssueRoleRef | null,
): boolean {
  return (
    left !== null &&
    right !== null &&
    left.type === right.type &&
    left.id === right.id
  );
}

export function reviewWorkflowViolation({
  previousCategory,
  nextCategory,
  executor,
  reviewer,
}: {
  previousCategory: IssueStatusCategory | null | undefined;
  nextCategory: IssueStatusCategory;
  executor: IssueRoleRef | null;
  reviewer: IssueRoleRef | null;
}): ReviewWorkflowViolation | null {
  if (EXECUTOR_REQUIRED_CATEGORIES.includes(nextCategory) && !executor) {
    return "executor_required";
  }
  if (!isReviewHandoff(previousCategory, nextCategory)) return null;
  if (!reviewer) return "reviewer_required";
  if (sameActor(executor, reviewer)) return "reviewer_must_differ";
  return null;
}
