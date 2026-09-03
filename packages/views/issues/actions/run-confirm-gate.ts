import type {
  Issue,
  IssueExecutorType,
  IssueReviewerType,
  IssueStatusCategory,
  UpdateIssueRequest,
} from "@patchbay/core/types";
import { issueStatusCategory } from "@patchbay/core/issues";
import { isIssueStatusCategory, type IssueStatusCatalog } from "@patchbay/core/issue-statuses";

/** The issue fields the gate reads. */
export type GateIssue = Pick<
  Issue,
  | "id"
  | "revision"
  | "status"
  | "status_category"
  | "executor_type"
  | "executor_id"
  | "reviewer_type"
  | "reviewer_id"
>;

/** Payload for the `issue-run-confirm` modal, or null when nothing to confirm. */
export type RunConfirmIntent =
  | {
      issueIds: [string];
      mode: "assign";
      executorType: "agent" | "team";
      executorId: string;
    }
  | {
      issueIds: [string];
      mode: "promote";
      status: string;
      executorType: "agent" | "team";
      executorId: string;
    }
  | {
      issueIds: [string];
      mode: "review";
      status: string;
      fromExecutorType: IssueExecutorType | null;
      fromExecutorId: string | null;
      executorType: IssueReviewerType | null;
      executorId: string | null;
      issueRevision?: number;
    }
  | {
      issueIds: [string];
      mode: "review-return";
      status: string;
      executorType: "agent" | "team";
      executorId: string;
      issueRevision?: number;
    };

/**
 * The category a status KEY belongs to — or `null` when nothing can answer.
 *
 * Three states, not two. `catalog.categoryOf` collapses "unknown custom key"
 * into `todo`, which is indistinguishable from a real `todo` and is exactly
 * the guess this gate must not make: it decides whether a write may start an
 * agent, so an unresolvable key has to stay unresolved and let the caller fail
 * safe. (MUL-6463)
 *
 * Resolution order mirrors the server (`issuestatus.Effective`): a category the
 * payload already carries wins, a BUILT-IN key is its own category, and only a
 * custom key needs the workspace catalog.
 */
export function resolveStatusCategory(
  statusKey: string,
  carriedCategory: IssueStatusCategory | undefined,
  catalog: Pick<IssueStatusCatalog, "entryOf">,
): IssueStatusCategory | null {
  const carried = issueStatusCategory({ status: statusKey, status_category: carriedCategory });
  if (carried) return carried;
  const category = catalog.entryOf(statusKey)?.category;
  return category && isIssueStatusCategory(category) ? category : null;
}

const RUNS_EXECUTOR_CATEGORIES: readonly IssueStatusCategory[] = [
  "todo",
  "in_progress",
  "in_review",
  "blocked",
];

function runsExecutor(category: IssueStatusCategory | null): boolean {
  return category !== null && RUNS_EXECUTOR_CATEGORIES.includes(category);
}

/**
 * Which confirmation, if any, an issue write needs before it is applied.
 *
 * Both writes that can hand work to an agent confirm; everything else applies
 * directly. Pure so every entry point — issue detail, context menu, table row —
 * routes on one answer instead of re-deriving it (MUL-6463).
 *
 * - **assign**: giving the issue an agent/team owner. Skipped only when the
 *   issue is KNOWN to be parked, because assigning into the backlog category
 *   never starts a run (`server/internal/service/issue_trigger.go`) and the
 *   dialog would promise something that cannot happen.
 * - **promote**: moving an already-owned issue out of the backlog category.
 *   That status change alone starts the run (`RunSourceStatus`), so it earns
 *   the same dialog — for built-in `todo` and every custom Todo-category
 *   status alike.
 *
 * Unresolvable categories fail toward confirming: a dialog the user dismisses
 * costs a click, a silent start costs an unwanted agent run.
 */
export function runConfirmIntent(
  issue: GateIssue,
  updates: Partial<UpdateIssueRequest>,
  catalog: Pick<IssueStatusCatalog, "entryOf">,
): RunConfirmIntent | null {
  const issueCategory = resolveStatusCategory(issue.status, issue.status_category, catalog);
  if (updates.status && updates.status !== issue.status) {
    const target = resolveStatusCategory(updates.status, undefined, catalog);
    const nextExecutorType = Object.prototype.hasOwnProperty.call(updates, "executor_type")
      ? updates.executor_type ?? null
      : issue.executor_type;
    const nextExecutorId = Object.prototype.hasOwnProperty.call(updates, "executor_id")
      ? updates.executor_id ?? null
      : issue.executor_id;
    if (
      issueCategory === "in_review" &&
      target === "in_progress" &&
      (issue.executor_type === "agent" || issue.executor_type === "team") &&
      !!issue.executor_id &&
      nextExecutorType === issue.executor_type &&
      nextExecutorId === issue.executor_id
    ) {
      return {
        issueIds: [issue.id],
        mode: "review-return",
        status: updates.status,
        executorType: issue.executor_type,
        executorId: issue.executor_id,
        issueRevision: issue.revision,
      };
    }
    if (target === "in_review" && issueCategory !== "in_review") {
      const reviewerWasProvided =
        Object.prototype.hasOwnProperty.call(updates, "reviewer_type") ||
        Object.prototype.hasOwnProperty.call(updates, "reviewer_id");
      const nextReviewerType = reviewerWasProvided
        ? updates.reviewer_type ?? null
        : issue.reviewer_type ?? null;
      const nextReviewerId = reviewerWasProvided
        ? updates.reviewer_id ?? null
        : issue.reviewer_id ?? null;
      const hasReviewer = !!(nextReviewerType && nextReviewerId);
      if (
        hasReviewer &&
        !(nextReviewerType === nextExecutorType && nextReviewerId === nextExecutorId)
      ) {
        return null;
      }
      return {
        issueIds: [issue.id],
        mode: "review",
        status: updates.status,
        fromExecutorType: issue.executor_type,
        fromExecutorId: issue.executor_id,
        executorType: nextReviewerType,
        executorId: nextReviewerId,
        issueRevision: issue.revision,
      };
    }
    if (
      (target === null || runsExecutor(target)) &&
      !runsExecutor(issueCategory) &&
      (nextExecutorType === "agent" || nextExecutorType === "team") &&
      nextExecutorId
    ) {
      return {
        issueIds: [issue.id],
        mode: "promote",
        status: updates.status,
        executorType: nextExecutorType,
        executorId: nextExecutorId,
      };
    }
  }

  if (
    (updates.executor_type === "agent" || updates.executor_type === "team") &&
    updates.executor_id &&
    (runsExecutor(issueCategory) || issueCategory === null)
  ) {
    return {
      issueIds: [issue.id],
      mode: "assign",
      executorType: updates.executor_type,
      executorId: updates.executor_id,
    };
  }

  return null;
}
