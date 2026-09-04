import { describe, expect, it } from "vitest";
import type { TimelineEntry } from "@patchbay/core/types";
import { formatActivity } from "./format-activity";

function activity(
  action: string,
  details: Record<string, unknown>,
): TimelineEntry {
  return {
    type: "activity",
    id: "activity-1",
    actor_type: "member",
    actor_id: "member-1",
    created_at: "2026-09-03T00:00:00Z",
    action,
    details,
  };
}

const actorName = (
  _type: string | null | undefined,
  id: string | null | undefined,
) => (id === "agent-2" ? "Build Agent" : "Alex");

describe("formatActivity role changes", () => {
  it("renders executor changes with explicit role terminology", () => {
    expect(
      formatActivity(
        activity("executor_changed", {
          from_type: "agent",
          from_id: "agent-1",
          to_type: "agent",
          to_id: "agent-2",
        }),
        actorName,
      ),
    ).toBe("set executor to Build Agent");
  });

  it("renders owner removal explicitly", () => {
    expect(
      formatActivity(
        activity("owner_changed", {
          from_type: "member",
          from_id: "member-2",
        }),
        actorName,
      ),
    ).toBe("removed owner");
  });

  it("distinguishes reviewer assignment from a review handoff", () => {
    expect(
      formatActivity(
        activity("review_handoff", {
          from_status: "in_review",
          to_status: "in_review",
          to_type: "agent",
          to_id: "agent-2",
        }),
        actorName,
      ),
    ).toBe("assigned reviewer to Build Agent");

    expect(
      formatActivity(
        activity("review_handoff", {
          from_status: "in_progress",
          to_status: "in_review",
          from_type: "agent",
          from_id: "agent-2",
          to_type: "member",
          to_id: "member-1",
        }),
        actorName,
      ),
    ).toBe("handed review from Build Agent to Alex");
  });

  it("renders reviewer replacement and removal with reviewer terminology", () => {
    expect(
      formatActivity(
        activity("review_handoff", {
          from_status: "in_review",
          to_status: "in_review",
          from_type: "member",
          from_id: "member-1",
          to_type: "agent",
          to_id: "agent-2",
        }),
        actorName,
      ),
    ).toBe("changed reviewer from Alex to Build Agent");

    expect(
      formatActivity(
        activity("review_handoff", {
          from_status: "in_review",
          to_status: "in_review",
          from_type: "agent",
          from_id: "agent-2",
        }),
        actorName,
      ),
    ).toBe("removed reviewer");
  });

  it("does not mistake a custom in-review status change for a handoff", () => {
    expect(
      formatActivity(
        activity("review_handoff", {
          from_status: "qa_pending",
          to_status: "qa_active",
          from_type: "member",
          from_id: "member-1",
          to_type: "agent",
          to_id: "agent-2",
        }),
        actorName,
        undefined,
        (status) =>
          status === "qa_pending" || status === "qa_active"
            ? "in_review"
            : "todo",
      ),
    ).toBe("changed reviewer from Alex to Build Agent");
  });

  it("does not claim a handoff when status evidence is missing or unknown", () => {
    expect(
      formatActivity(
        activity("review_handoff", {
          from_type: "member",
          from_id: "member-1",
          to_type: "agent",
          to_id: "agent-2",
        }),
        actorName,
      ),
    ).toBe("changed reviewer from Alex to Build Agent");

    expect(
      formatActivity(
        activity("review_handoff", {
          from_status: "future_a",
          to_status: "future_b",
          from_type: "member",
          from_id: "member-1",
          to_type: "agent",
          to_id: "agent-2",
        }),
        actorName,
      ),
    ).toBe("changed reviewer from Alex to Build Agent");
  });

  it("localizes review handoff wording without changing unknown-event fallback", () => {
    expect(
      formatActivity(
        activity("review_handoff", {
          from_status: "in_progress",
          to_status: "in_review",
          from_type: "agent",
          from_id: "agent-2",
          to_type: "member",
          to_id: "member-1",
        }),
        actorName,
        undefined,
        undefined,
        "zh-CN",
      ),
    ).toBe("将审核从 Build Agent 移交给 Alex");
    expect(
      formatActivity(
        activity("future_review_activity", {}),
        actorName,
        undefined,
        undefined,
        "zh-CN",
      ),
    ).toBe("future_review_activity");
  });

  it("ignores malformed optional detail values without throwing", () => {
    expect(
      formatActivity(
        activity("team_leader_evaluated", {
          outcome: "action",
          reason: 42,
        }),
        actorName,
      ),
    ).toBe("evaluated and took action");

    expect(
      formatActivity(
        activity("priority_changed", {
          from: "none",
          to: "constructor",
        }),
        actorName,
      ),
    ).toBe("changed priority: No priority → constructor");
  });

  it("preserves the raw action for unknown activity events", () => {
    expect(formatActivity(activity("future_activity", {}), actorName)).toBe(
      "future_activity",
    );
  });
});
