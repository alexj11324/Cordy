import { beforeAll, describe, expect, it } from "vitest";
import { ApiClient, setApiInstance } from "@orvilo/core/api";
import type { Invitation, MemberWithUser } from "@orvilo/core/types";
import {
  toDirectoryInvitation,
  toDirectoryMember,
} from "./team-directory-data";

beforeAll(() => {
  setApiInstance(new ApiClient("https://api.example.test"));
});

describe("team directory row adapters", () => {
  it("keeps real member identity and renders a stable display name", () => {
    const member: MemberWithUser = {
      id: "member-1",
      workspace_id: "workspace-1",
      user_id: "user-1",
      role: "admin",
      created_at: "2026-04-17T12:00:00.000Z",
      name: "Lena Torres",
      email: "lena@example.com",
      avatar_url: null,
    };

    expect(toDirectoryMember(member, "en-US")).toEqual({
      id: "member-1",
      userId: "user-1",
      fullName: "Lena Torres",
      displayName: "lena",
      email: "lena@example.com",
      role: "admin",
      status: "active",
      joinedAt: "Apr 17, 2026",
      joinedAtTimestamp: "2026-04-17T12:00:00.000Z",
      avatarSrc: null,
      initials: "LT",
    });
  });

  it("preserves invitation status and inviter details", () => {
    const invitation: Invitation = {
      id: "invitation-1",
      workspace_id: "workspace-1",
      inviter_id: "user-1",
      invitee_email: "remy@example.com",
      invitee_user_id: null,
      role: "member",
      status: "pending",
      created_at: "2026-05-02T12:00:00.000Z",
      updated_at: "2026-05-02T12:00:00.000Z",
      expires_at: "2026-05-09T12:00:00.000Z",
      inviter_name: "Lena Torres",
      inviter_email: "lena@example.com",
    };

    expect(toDirectoryInvitation(invitation, "en-US")).toEqual({
      id: "invitation-1",
      email: "remy@example.com",
      handle: "remy",
      role: "member",
      invitedBy: "Lena Torres",
      sentAt: "May 2, 2026",
      status: "pending",
      sentAtTimestamp: "2026-05-02T12:00:00.000Z",
    });
  });

  it("resolves site-relative member avatars against the API origin", () => {
    const member: MemberWithUser = {
      id: "member-2",
      workspace_id: "workspace-1",
      user_id: "user-2",
      role: "member",
      created_at: "2026-04-17T12:00:00.000Z",
      name: "Mina Park",
      email: "mina@example.com",
      avatar_url: "/uploads/members/avatar.png",
    };

    expect(toDirectoryMember(member, "en-US").avatarSrc).toBe(
      "https://api.example.test/uploads/members/avatar.png",
    );
  });
});
