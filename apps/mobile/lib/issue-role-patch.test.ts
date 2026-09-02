import { describe, expect, it } from "vitest";
import {
  executorPatch,
  ownerPatch,
  reviewerPatch,
} from "./issue-role-patch";

/**
 * The point of these tests is the NEGATIVE space: each patch must be missing
 * the other two roles' keys entirely. `useUpdateIssue` spreads the patch over
 * the cached issue, so an extra key set to null would read as "clear that
 * role" and wipe a value the user never touched.
 */
const OWNER_KEYS = ["owner_type", "owner_id"];
const EXECUTOR_KEYS = ["executor_type", "executor_id"];
const REVIEWER_KEYS = ["reviewer_type", "reviewer_id"];

describe("issue role patches", () => {
  it("writes an owner without naming executor or reviewer keys", () => {
    const patch = ownerPatch({ type: "member", id: "member-1" });
    expect(patch).toEqual({ owner_type: "member", owner_id: "member-1" });
    expect(Object.keys(patch).sort()).toEqual([...OWNER_KEYS].sort());
  });

  it("writes an executor without naming owner or reviewer keys", () => {
    const patch = executorPatch({ type: "agent", id: "agent-1" });
    expect(patch).toEqual({ executor_type: "agent", executor_id: "agent-1" });
    expect(Object.keys(patch).sort()).toEqual([...EXECUTOR_KEYS].sort());
  });

  it("writes a reviewer without naming owner or executor keys", () => {
    const patch = reviewerPatch({ type: "team", id: "team-1" });
    expect(patch).toEqual({ reviewer_type: "team", reviewer_id: "team-1" });
    expect(Object.keys(patch).sort()).toEqual([...REVIEWER_KEYS].sort());
  });

  it("clears only its own role, leaving the other two untouched", () => {
    const issue = {
      owner_type: "member",
      owner_id: "member-1",
      executor_type: "agent",
      executor_id: "agent-1",
      reviewer_type: "team",
      reviewer_id: "team-1",
    };

    // This mirrors the optimistic merge in `useUpdateIssue.onMutate`.
    expect({ ...issue, ...ownerPatch(null) }).toEqual({
      ...issue,
      owner_type: null,
      owner_id: null,
    });
    expect({ ...issue, ...executorPatch(null) }).toEqual({
      ...issue,
      executor_type: null,
      executor_id: null,
    });
    expect({ ...issue, ...reviewerPatch(null) }).toEqual({
      ...issue,
      reviewer_type: null,
      reviewer_id: null,
    });
  });

  it("reassigning one role preserves the other two through the merge", () => {
    const issue = {
      owner_type: "member" as const,
      owner_id: "member-1",
      executor_type: "agent" as const,
      executor_id: "agent-1",
      reviewer_type: "team" as const,
      reviewer_id: "team-1",
    };

    const afterOwner = {
      ...issue,
      ...ownerPatch({ type: "member", id: "member-2" }),
    };
    expect(afterOwner.executor_id).toBe("agent-1");
    expect(afterOwner.reviewer_id).toBe("team-1");

    const afterExecutor = {
      ...afterOwner,
      ...executorPatch({ type: "team", id: "team-2" }),
    };
    expect(afterExecutor.owner_id).toBe("member-2");
    expect(afterExecutor.reviewer_id).toBe("team-1");

    const afterReviewer = {
      ...afterExecutor,
      ...reviewerPatch({ type: "member", id: "member-3" }),
    };
    expect(afterReviewer.owner_id).toBe("member-2");
    expect(afterReviewer.executor_type).toBe("team");
    expect(afterReviewer.executor_id).toBe("team-2");
  });

  it("refuses a non-member owner instead of writing an invalid owner type", () => {
    // The shared role picker is typed over the wider actor union; owner is
    // member-only, so an agent selection must clear rather than persist.
    expect(ownerPatch({ type: "agent", id: "agent-1" })).toEqual({
      owner_type: null,
      owner_id: null,
    });
  });
});
