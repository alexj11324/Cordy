import type { MemberRole } from "@orvilo/core/types";

export type MemberActionPermissionInput = {
  canManage: boolean;
  canManageOwners: boolean;
  isSelf: boolean;
  memberRole: MemberRole;
};

export function memberActionAvailability({
  canManage,
  canManageOwners,
  isSelf,
  memberRole,
}: MemberActionPermissionInput): {
  canEditRole: boolean;
  canRemove: boolean;
} {
  const canManageTarget =
    canManage && !isSelf && (memberRole !== "owner" || canManageOwners);
  return { canEditRole: canManageTarget, canRemove: canManageTarget };
}

export function memberRoleOptions({
  memberRole,
  canManageOwners,
  ownerCount,
}: {
  memberRole: MemberRole;
  canManageOwners: boolean;
  ownerCount: number;
}): Array<{ role: MemberRole; disabled: boolean }> {
  const roles: MemberRole[] = canManageOwners
    ? ["owner", "admin", "member"]
    : ["admin", "member"];
  const isLastOwner = memberRole === "owner" && ownerCount <= 1;
  return roles.map((role) => ({
    role,
    disabled: isLastOwner && role !== "owner",
  }));
}
