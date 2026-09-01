import type { Issue, IssueActorType } from "../types/issue";

export type IssueAssigneeRef = { type: IssueActorType; id: string };

type IssueRoleFields = Pick<
  Issue,
  "owner_type" | "owner_id" | "executor_type" | "executor_id"
>;

/** Who the issue is assigned to for grouping, filters, and avatars: executor if set, otherwise owner. */
export function issueAssigneeRef(issue: IssueRoleFields): IssueAssigneeRef | null {
  if (issue.executor_type && issue.executor_id) {
    return { type: issue.executor_type, id: issue.executor_id };
  }
  if (issue.owner_type && issue.owner_id) {
    return { type: issue.owner_type, id: issue.owner_id };
  }
  return null;
}
