import type {
  Issue,
  IssueAssigneeType,
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
  | "assignee_type"
  | "assignee_id"
  | "reviewer_type"
  | "reviewer_id"
>;

/** Payload for the `issue-run-confirm` modal, or null when nothing to confirm. */
export type RunConfirmIntent =
  | {
      issueIds: [string];
      mode: "assign";
      assigneeType: "agent" | "team";
      assigneeId: string;
    }
  | {
      issueIds: [string];
      mode: "promote";
      status: string;
      assigneeType: "agent" | "team";
      assigneeId: string;
    }
  | {
      issueIds: [string];
      mode: "review";
      status: string;
      fromAssigneeType: IssueAssigneeType | null;
      fromAssigneeId: string | null;
      assigneeType: IssueAssigneeType | null;
      assigneeId: string | null;
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

/** Categories a promotion can land in without starting a run. */
const NEVER_STARTS = ["backlog", "done", "cancelled"];

/**
 * Which confirmation, if any, an issue write needs before it is applied.
 *
 * Both writes that can hand work to an agent confirm; everything else applies
 * directly. Pure so every entry point — issue detail, context menu, table row —
 * routes on one answer instead of re-deriving it (PB-6463).
 *
 * - **assign**: giving the issue an agent/team owner. Skipped only when the
 *   issue is KNOWN to be parked, because assigning into the backlog category
 *   never starts a run (the Rust issue-trigger service) and the
 *   dialog would promise something that cannot happen.
 * - **promote**: moving an already-owned issue out of the backlog category.
 *   That status change alone starts the run (`RunSourceStatus`), so it earns
 *   the same dialog — for built-in `todo` and every custom Todo-category
 *   status alike.
 * - **review**: entering the Review category. The dialog requires a reviewer
 *   different from the current owner and sends status + reviewer atomically.
 *   If a reviewer is already on the issue (or supplied in the same write),
 *   the write applies directly.
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
  const parked = issueCategory === "backlog";

  if (updates.status && updates.status !== issue.status) {
    const target = resolveStatusCategory(updates.status, undefined, catalog);
    if (target === "in_review" && issueCategory !== "in_review") {
      const nextOwnerType = Object.prototype.hasOwnProperty.call(updates, "assignee_type")
        ? updates.assignee_type ?? null
        : issue.assignee_type;
      const nextOwnerId = Object.prototype.hasOwnProperty.call(updates, "assignee_id")
        ? updates.assignee_id ?? null
        : issue.assignee_id;
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
      const sameAsOwner =
        hasReviewer &&
        nextReviewerType === nextOwnerType &&
        nextReviewerId === nextOwnerId;
      if (hasReviewer && !sameAsOwner) {
        return null;
      }
      return {
        issueIds: [issue.id],
        mode: "review",
        status: updates.status,
        fromAssigneeType: issue.assignee_type,
        fromAssigneeId: issue.assignee_id,
        assigneeType: nextReviewerType,
        assigneeId: nextReviewerId,
        issueRevision: issue.revision,
      };
    }
  }

  if (
    (updates.assignee_type === "agent" || updates.assignee_type === "team") &&
    updates.assignee_id &&
    !parked
  ) {
    return {
      issueIds: [issue.id],
      mode: "assign",
      assigneeType: updates.assignee_type,
      assigneeId: updates.assignee_id,
    };
  }

  const owner = issue.assignee_type;
  if (
    updates.status &&
    updates.status !== issue.status &&
    // Unknown counts as possibly-parked: the write may promote, so confirm.
    (parked || issueCategory === null) &&
    (owner === "agent" || owner === "team") &&
    issue.assignee_id
  ) {
    const target = resolveStatusCategory(updates.status, undefined, catalog);
    // An unresolvable TARGET is possibly-active for the same reason.
    if (target === null || !NEVER_STARTS.includes(target)) {
      return {
        issueIds: [issue.id],
        mode: "promote",
        status: updates.status,
        assigneeType: owner,
        assigneeId: issue.assignee_id,
      };
    }
  }

  return null;
}
