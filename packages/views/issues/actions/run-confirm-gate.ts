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
 * safe. (PB-6463)
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

/**
 * Which confirmation, if any, an issue write needs before it is applied.
 *
 * Both writes that can hand work to an agent confirm; everything else applies
 * directly. Pure so every entry point — issue detail, context menu, table row —
 * routes on one answer instead of re-deriving it (PB-6463).
 *
 * - **assign**: changing the executor while the issue is In Progress.
 * - **promote**: admitting an already-executable issue into In Progress.
 *   Backlog, Todo and Blocked never start an agent directly.
 * - **review**: entering the Review category. The dialog requires a reviewer
 *   different from the current owner and sends status + reviewer atomically.
 *   If a reviewer is already on the issue (or supplied in the same write),
 *   the write applies directly.
 * - **review-return**: returning from Review to In Progress. The existing
 *   implementation owner is restored by the durable coordinator, so the
 *   dialog preserves the handoff-note and suppress-run choices before the
 *   status write is applied. Redundant executor fields from grouped surfaces
 *   do not turn this into a different assignment.
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
      const sameAsExecutor =
        hasReviewer &&
        nextReviewerType === nextExecutorType &&
        nextReviewerId === nextExecutorId;
      if (hasReviewer && !sameAsExecutor) {
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
      (target === "in_progress" || target === null) &&
      issueCategory !== "in_progress" &&
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
    (issueCategory === "in_progress" || issueCategory === null)
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
