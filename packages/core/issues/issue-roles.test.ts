import { describe, expect, it } from "vitest";
import { issueExecutorRef } from "./issue-roles";

describe("issueExecutorRef", () => {
  it("returns the executor", () => {
    expect(
      issueExecutorRef({
        executor_type: "agent",
        executor_id: "a1",
      }),
    ).toEqual({ type: "agent", id: "a1" });
  });

  it("returns null when there is no executor", () => {
    expect(
      issueExecutorRef({
        executor_type: null,
        executor_id: null,
      }),
    ).toBeNull();
  });
});
