import type { Issue, IssueActorType } from "@patchbay/core/types";
import type { IssuesScope } from "@/data/stores/issues-view-store";

type IssueRoleFields = Pick<
  Issue,
  | "owner_type"
  | "owner_id"
  | "executor_type"
  | "executor_id"
  | "reviewer_type"
  | "reviewer_id"
>;

export type IssueRole = "owner" | "executor" | "reviewer";
export type IssueRoleState =
  | { kind: "unassigned" }
  | { kind: "unknown" }
  | { kind: "assigned"; actor: { type: IssueActorType; id: string } };

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

export function issueRoleState(
  issue: IssueRoleFields,
  role: IssueRole,
): IssueRoleState {
  const [type, id] =
    role === "owner"
      ? [issue.owner_type, issue.owner_id]
      : role === "executor"
        ? [issue.executor_type, issue.executor_id]
        : [issue.reviewer_type, issue.reviewer_id];
  if (!type && !id) return { kind: "unassigned" };
  if (typeof type !== "string" || typeof id !== "string" || id.length === 0) {
    return { kind: "unknown" };
  }
  const validType =
    role === "owner"
      ? type === "member"
      : role === "executor"
        ? type === "agent" || type === "team"
        : type === "member" || type === "agent" || type === "team";
  return validType
    ? { kind: "assigned", actor: { type, id } }
    : { kind: "unknown" };
}

/** Return only the explicitly requested role's actor, without a fallback. */
export function issueActorForRole(
  issue: IssueRoleFields,
  role: IssueRole,
): { type: IssueActorType; id: string } | null {
  const state = issueRoleState(issue, role);
  return state.kind === "assigned" ? state.actor : null;
}
