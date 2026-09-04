import { describe, expect, it } from "vitest";
import { resolveInboxRoleActorType } from "./inbox-role";

describe("inbox issue role actor types", () => {
  it("only infers members for omitted or explicit member owner types", () => {
    expect(resolveInboxRoleActorType("owner", undefined)).toBe("member");
    expect(resolveInboxRoleActorType("owner", "member")).toBe("member");
    expect(resolveInboxRoleActorType("owner", "agent")).toBeNull();
    expect(resolveInboxRoleActorType("owner", "team")).toBeNull();
    expect(resolveInboxRoleActorType("owner", "future_actor")).toBeNull();
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
