import type { IssueActorType } from "@patchbay/core/types";

export type InboxIssueRole = "owner" | "executor";

/**
 * Resolve an inbox role's actor type without inferring one role from another.
 * Owners are always members; executors are only agents or teams. An absent or
 * legacy executor type stays unknown instead of being relabeled as a member.
 */
export function resolveInboxRoleActorType(
  role: InboxIssueRole,
  rawType: string | undefined,
): IssueActorType | null {
  if (role === "owner") return "member";
  return rawType === "agent" || rawType === "team" ? rawType : null;
}
