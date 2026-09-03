import { describe, expect, it } from "vitest";
import type { TimelineEntry } from "@patchbay/core/types";
import { formatActivity } from "./format-activity";

function activity(action: string, details: Record<string, string>): TimelineEntry {
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

const actorName = (_type: string | null | undefined, id: string | null | undefined) =>
  id === "agent-2" ? "Build Agent" : "Alex";

describe("formatActivity role changes", () => {
  it("renders executor changes without assignee terminology", () => {
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
});
