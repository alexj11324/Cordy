import { describe, expect, it } from "vitest";
import { issueAssigneeRef } from "./issue-roles";

describe("issueAssigneeRef", () => {
  it("prefers executor over owner", () => {
    expect(
      issueAssigneeRef({
        owner_type: "member",
        owner_id: "u1",
        executor_type: "agent",
        executor_id: "a1",
      }),
    ).toEqual({ type: "agent", id: "a1" });
  });

  it("falls back to owner when there is no executor", () => {
    expect(
      issueAssigneeRef({
        owner_type: "member",
        owner_id: "u1",
        executor_type: null,
        executor_id: null,
      }),
    ).toEqual({ type: "member", id: "u1" });
  });

  it("returns null when both roles are empty", () => {
    expect(
      issueAssigneeRef({
        owner_type: null,
        owner_id: null,
        executor_type: null,
        executor_id: null,
      }),
    ).toBeNull();
  });
});
