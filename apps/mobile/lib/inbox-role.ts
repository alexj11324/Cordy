import type { IssueActorType } from "@patchbay/core/types";

export type InboxIssueRole = "owner" | "executor";

/**
 * Resolve an inbox role's actor type without inferring one role from another.
 * Owners accept an omitted legacy type or an explicit member type; executors
 * are only agents or teams. Invalid or unknown types stay unresolved instead
 * of being relabeled as a member.
 */
export function resolveInboxRoleActorType(
  role: InboxIssueRole,
  rawType: string | undefined,
): IssueActorType | null {
  if (role === "owner") {
    return rawType === undefined || rawType === "member" ? "member" : null;
  }
  return rawType === "agent" || rawType === "team" ? rawType : null;
}
