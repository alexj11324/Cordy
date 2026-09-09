import type {
  Invitation,
  MemberRole,
  MemberWithUser,
} from "@orvilo/core/types";
import { resolvePublicFileUrl } from "@orvilo/core/workspace/avatar-url";

export type DirectoryMember = {
  id: string;
  userId: string;
  fullName: string;
  displayName: string;
  email: string;
  role: MemberRole;
  status: "active";
  joinedAt: string;
  joinedAtTimestamp: string;
  avatarSrc: string | null;
  initials: string;
};

export type DirectoryInvitation = {
  id: string;
  email: string;
  handle: string;
  role: MemberRole;
  invitedBy: string;
  sentAt: string;
  status: Invitation["status"];
  sentAtTimestamp: string;
};

function initialsFor(value: string): string {
  return (
    value
      .trim()
      .split(/\s+/)
      .map((part) => part.charAt(0))
      .join("")
      .toUpperCase()
      .slice(0, 2) || "U"
  );
}

function formatDate(value: string, locale: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "—"
    : new Intl.DateTimeFormat(locale, {
        year: "numeric",
        month: "short",
        day: "numeric",
      }).format(date);
}

export function toDirectoryMember(
  member: MemberWithUser,
  locale: string,
): DirectoryMember {
  return {
    id: member.id,
    userId: member.user_id,
    fullName: member.name,
    displayName: member.email.split("@")[0] ?? member.user_id,
    email: member.email,
    role: member.role,
    status: "active",
    joinedAt: formatDate(member.created_at, locale),
    joinedAtTimestamp: member.created_at,
    avatarSrc: resolvePublicFileUrl(member.avatar_url),
    initials: initialsFor(member.name),
  };
}

export function toDirectoryInvitation(
  invitation: Invitation,
  locale: string,
): DirectoryInvitation {
  return {
    id: invitation.id,
    email: invitation.invitee_email,
    handle: invitation.invitee_email.split("@")[0] ?? invitation.invitee_email,
    role: invitation.role,
    invitedBy:
      invitation.inviter_name ??
      invitation.inviter_email ??
      invitation.inviter_id,
    sentAt: formatDate(invitation.created_at, locale),
    status: invitation.status,
    sentAtTimestamp: invitation.created_at,
  };
}
