import type { Issue, IssueActorType } from "@patchbay/core/types";
import type { IssuesScope } from "@/data/stores/issues-view-store";

type IssueRoleFields = Pick<
  Issue,
  "owner_type" | "owner_id" | "executor_type" | "executor_id"
>;

export type IssueRole = "owner" | "executor";

/**
 * Workspace issue scopes use the role column that names the scope. A member
 * scope is about ownership; an agents scope is about execution. These are
 * independent issue roles, so an executor must never stand in for a missing
 * owner (or vice versa).
 */
export function issueMatchesScope(
  issue: Pick<Issue, "owner_type" | "executor_type">,
  scope: IssuesScope,
): boolean {
  switch (scope) {
    case "members":
      return issue.owner_type === "member";
    case "agents":
      return issue.executor_type === "agent" || issue.executor_type === "team";
    case "all":
      return true;
  }
}

export function filterIssuesByScope<
  T extends Pick<Issue, "owner_type" | "executor_type">,
>(issues: T[], scope: IssuesScope): T[] {
  return issues.filter((issue) => issueMatchesScope(issue, scope));
}

/** Return only the explicitly requested role's actor, without a fallback. */
export function issueActorForRole(
  issue: IssueRoleFields,
  role: IssueRole,
): { type: IssueActorType; id: string } | null {
  if (role === "owner") {
    return issue.owner_type && issue.owner_id
      ? { type: issue.owner_type, id: issue.owner_id }
      : null;
  }

  return issue.executor_type && issue.executor_id
    ? { type: issue.executor_type, id: issue.executor_id }
    : null;
}
