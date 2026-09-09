// @vitest-environment node

import { describe, expect, it } from "vitest";
import {
  memberActionAvailability,
  memberRoleOptions,
} from "./team-directory-permissions";

describe("team directory member permissions", () => {
  it("lets an owner manage a co-owner and transfer ownership", () => {
    expect(
      memberActionAvailability({
        canManage: true,
        canManageOwners: true,
        isSelf: false,
        memberRole: "owner",
      }),
    ).toEqual({ canEditRole: true, canRemove: true });
    expect(
      memberRoleOptions({
        memberRole: "owner",
        canManageOwners: true,
        ownerCount: 2,
      }),
    ).toEqual([
      { role: "owner", disabled: false },
      { role: "admin", disabled: false },
      { role: "member", disabled: false },
    ]);
  });

  it("blocks demoting the last owner and hides owner promotion for admins", () => {
    expect(
      memberRoleOptions({
        memberRole: "owner",
        canManageOwners: true,
        ownerCount: 1,
      }),
    ).toEqual([
      { role: "owner", disabled: false },
      { role: "admin", disabled: true },
      { role: "member", disabled: true },
    ]);
    expect(
      memberRoleOptions({
        memberRole: "member",
        canManageOwners: false,
        ownerCount: 2,
      }),
    ).toEqual([
      { role: "admin", disabled: false },
      { role: "member", disabled: false },
    ]);
  });
});
