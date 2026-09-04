import { describe, expect, it } from "vitest";
import type { InboxItem } from "@patchbay/core/types";
import { inboxDetailText } from "./inbox-detail-text";

function item(type: string, details: Record<string, string> = {}): InboxItem {
  return {
    id: "inbox-1",
    workspace_id: "workspace-1",
    recipient_type: "member",
    recipient_id: "member-1",
    actor_type: "member",
    actor_id: "member-2",
    type: type as InboxItem["type"],
    severity: "info",
    issue_id: "issue-1",
    title: "Review the migration",
    body: null,
    issue_status: "in_review",
    read: false,
    archived: false,
    created_at: "2026-09-03T00:00:00Z",
    details,
  };
}

const actorName = (_type: string | null | undefined, id: string) =>
  id === "agent-1" ? "Build Agent" : "Alex";

describe("inbox detail text", () => {
  it("labels a review request with reviewer terminology", () => {
    expect(
      inboxDetailText(
        item("review_requested", {
          new_reviewer_type: "agent",
          new_reviewer_id: "agent-1",
        }),
        actorName,
      ),
    ).toBe("Review requested for Build Agent");
  });

  it("keeps executor assignment wording separate", () => {
    expect(
      inboxDetailText(
        item("issue_assigned", {
          new_executor_type: "agent",
          new_executor_id: "agent-1",
        }),
        actorName,
      ),
    ).toBe("Set executor to Build Agent");
  });

  it("renders an unknown inbox type as its raw value", () => {
    expect(inboxDetailText(item("future_review_event"), actorName)).toBe(
      "future_review_event",
    );
    expect(inboxDetailText(item("constructor"), actorName)).toBe("constructor");
  });
});
