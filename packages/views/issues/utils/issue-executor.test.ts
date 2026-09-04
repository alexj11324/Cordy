// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { Issue } from "@patchbay/core/types";
import { getIssueExecutor } from "./issue-executor";

describe("getIssueExecutor", () => {
  it("returns the explicit executor", () => {
    expect(
      getIssueExecutor({ executor_type: "team", executor_id: "team-1" }),
    ).toEqual({ type: "team", id: "team-1" });
  });

  it("does not reinterpret an owner-only issue as executed", () => {
    const issue = {
      owner_type: "member",
      owner_id: "member-1",
      executor_type: null,
      executor_id: null,
    } as Pick<
      Issue,
      "owner_type" | "owner_id" | "executor_type" | "executor_id"
    >;

    expect(getIssueExecutor(issue)).toBeNull();
  });

  it("rejects a partial executor pair", () => {
    expect(
      getIssueExecutor({ executor_type: "agent", executor_id: null }),
    ).toBeNull();
    expect(
      getIssueExecutor({ executor_type: null, executor_id: "agent-1" }),
    ).toBeNull();
  });
});
