import { describe, expect, it } from "vitest";
import { resolveInboxRoleActorType } from "./inbox-role";

describe("inbox issue role actor types", () => {
  it("keeps owners member-only even when the payload omits the type", () => {
    expect(resolveInboxRoleActorType("owner", undefined)).toBe("member");
    expect(resolveInboxRoleActorType("owner", "agent")).toBe("member");
  });

  it("accepts both explicit executor actor types", () => {
    expect(resolveInboxRoleActorType("executor", "agent")).toBe("agent");
    expect(resolveInboxRoleActorType("executor", "team")).toBe("team");
  });

  it("does not relabel an absent or member executor as a member actor", () => {
    expect(resolveInboxRoleActorType("executor", undefined)).toBeNull();
    expect(resolveInboxRoleActorType("executor", "member")).toBeNull();
  });
});
