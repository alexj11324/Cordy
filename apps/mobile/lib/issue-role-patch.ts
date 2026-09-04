/**
 * Pure builders for the three independent issue role writes.
 *
 * Owner, executor and reviewer are separate columns on the issue, not one
 * collapsed assignment. `useUpdateIssue` merges a patch into the
 * detail cache with a bare spread (`{...prev, ...patch}`), so a patch that
 * carries a key it did not mean to change silently clobbers that role — and
 * the optimistic cache is what every chip and list renders until the server
 * response lands. Keeping the payloads here, pure and tested, is what makes
 * "an owner write cannot touch the executor" a checkable property rather than
 * a convention three route files each have to remember.
 *
 * Each builder returns ONLY its own `*_type` / `*_id` pair. Clearing a role
 * sends explicit nulls (the server distinguishes absent from null), which is
 * why the return type is exact rather than a partial spread of the request.
 */
import type {
  CreateIssueRequest,
  IssueExecutorType,
  IssueOwnerType,
  IssueReviewerType,
} from "@patchbay/core/types";
import type { ExecutorValue } from "@/components/issue/pickers/executor-picker-body";
import type { RoleValue } from "@/lib/issue-role-options";

export type OwnerPatch = {
  owner_type: IssueOwnerType | null;
  owner_id: string | null;
};

export type ExecutorPatch = {
  executor_type: IssueExecutorType | null;
  executor_id: string | null;
};

export type ReviewerPatch = {
  reviewer_type: IssueReviewerType | null;
  reviewer_id: string | null;
};

/**
 * Owner is member-only (`IssueOwnerType`). The shared role picker is typed
 * over the wider actor union, so a non-member selection is rejected here
 * rather than sent as an owner the server would refuse.
 */
export function ownerPatch(next: RoleValue): OwnerPatch {
  if (next?.type !== "member") return { owner_type: null, owner_id: null };
  return { owner_type: "member", owner_id: next.id };
}

export function executorPatch(next: ExecutorValue): ExecutorPatch {
  if (!next) return { executor_type: null, executor_id: null };
  return { executor_type: next.type, executor_id: next.id };
}

export function reviewerPatch(next: RoleValue): ReviewerPatch {
  if (!next) return { reviewer_type: null, reviewer_id: null };
  return { reviewer_type: next.type, reviewer_id: next.id };
}

type IssueRoleCreateFields = Partial<
  Pick<
    CreateIssueRequest,
    | "owner_type"
    | "owner_id"
    | "executor_type"
    | "executor_id"
    | "reviewer_type"
    | "reviewer_id"
  >
>;

export function issueRoleCreateFields(
  owner: RoleValue,
  executor: ExecutorValue,
  reviewer: RoleValue,
): IssueRoleCreateFields {
  return {
    ...(owner?.type === "member"
      ? { owner_type: "member" as const, owner_id: owner.id }
      : {}),
    ...(executor
      ? { executor_type: executor.type, executor_id: executor.id }
      : {}),
    ...(reviewer
      ? { reviewer_type: reviewer.type, reviewer_id: reviewer.id }
      : {}),
  };
}
