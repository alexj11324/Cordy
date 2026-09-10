import type {
  IssueActorType,
  IssueReviewSubmission,
  IssueStatus,
  IssueStatusCategory,
} from "@orvilo/core/types";

export type IssueRoleRef = { type: IssueActorType; id: string };

export type ReviewHandoffPatch = {
  status: IssueStatus;
  reviewer_type: IssueActorType;
  reviewer_id: string;
};

export type ReviewSubmissionDraft = {
  worktree: string;
  branch: string;
  commit: string;
  pullRequests: string;
};

export type ReviewSubmissionPatch = ReviewHandoffPatch & {
  review_submission: IssueReviewSubmission;
};

export type ReviewWorkflowViolation =
  | "executor_required"
  | "reviewer_required"
  | "reviewer_must_differ";

export type IssueStatusSelectionPlan =
  | { kind: "apply"; status: IssueStatus }
  | { kind: "collect_review_evidence"; status: IssueStatus }
  | { kind: "choose_reviewer"; status: IssueStatus }
  | { kind: "blocked"; violation: "executor_required" };

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

const FULL_COMMIT_PATTERN = /^[0-9a-f]{40}([0-9a-f]{24})?$/i;
const PULL_REQUEST_PATH_PATTERN = /\/(pull|pulls|merge_requests)\/\d+\/?$/;

function normalizePullRequest(raw: string): string | null {
  try {
    const url = new URL(raw.trim());
    if (
      (url.protocol !== "https:" && url.protocol !== "http:") ||
      !url.hostname ||
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      !PULL_REQUEST_PATH_PATTERN.test(url.pathname)
    ) {
      return null;
    }
    return url.toString().replace(/\/$/, raw.trim().endsWith("/") ? "/" : "");
  } catch {
    return null;
  }
}

/** Build the same complete review handoff accepted by the shared API. */
export function buildReviewSubmissionPatch(
  status: IssueStatus,
  reviewer: IssueRoleRef,
  draft: ReviewSubmissionDraft,
): ReviewSubmissionPatch | null {
  const worktree = draft.worktree.trim();
  const branch = draft.branch.trim();
  const commit = draft.commit.trim();
  const pullRequests = draft.pullRequests
    .split(/\r?\n/)
    .map(normalizePullRequest)
    .filter((url): url is string => url !== null);
  const submittedLineCount = draft.pullRequests
    .split(/\r?\n/)
    .filter((line) => line.trim()).length;

  if (
    !worktree ||
    !branch ||
    !FULL_COMMIT_PATTERN.test(commit) ||
    pullRequests.length === 0 ||
    pullRequests.length !== submittedLineCount
  ) {
    return null;
  }

  return {
    ...reviewHandoffPatch(status, reviewer),
    review_submission: {
      worktree,
      branch,
      commit,
      pull_requests: pullRequests,
    },
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

/**
 * Translate the workflow gate into the UI outcomes shared by existing
 * and draft issue status pickers. Entering review never writes status first:
 * the reviewer picker completes the status + reviewer pair together.
 */
export function planIssueStatusSelection({
  previousCategory,
  nextStatus,
  nextCategory,
  executor,
  reviewer,
}: {
  previousCategory: IssueStatusCategory | null | undefined;
  nextStatus: IssueStatus;
  nextCategory: IssueStatusCategory;
  executor: IssueRoleRef | null;
  reviewer: IssueRoleRef | null;
}): IssueStatusSelectionPlan {
  const violation = reviewWorkflowViolation({
    previousCategory,
    nextCategory,
    executor,
    reviewer,
  });
  if (violation === "executor_required") {
    return { kind: "blocked", violation };
  }
  if (
    violation === "reviewer_required" ||
    violation === "reviewer_must_differ"
  ) {
    return { kind: "choose_reviewer", status: nextStatus };
  }
  if (isReviewHandoff(previousCategory, nextCategory)) {
    return { kind: "collect_review_evidence", status: nextStatus };
  }
  return { kind: "apply", status: nextStatus };
}
