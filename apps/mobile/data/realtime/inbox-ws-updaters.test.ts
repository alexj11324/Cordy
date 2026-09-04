import { QueryClient } from "@tanstack/react-query";
import type { InboxItem } from "@patchbay/core/types";
import { describe, expect, it, vi } from "vitest";
import { inboxKeys } from "@/data/queries/inbox";
import { patchInboxIssueStatus } from "./inbox-ws-updaters";

vi.mock("@/data/api", () => ({ api: {} }));

describe("inbox issue realtime projection", () => {
  it("updates review status without losing review-request details", () => {
    const qc = new QueryClient();
    const key = inboxKeys.list("workspace-1");
    const item = {
      id: "inbox-1",
      workspace_id: "workspace-1",
      recipient_type: "member",
      recipient_id: "reviewer-1",
      actor_type: "member",
      actor_id: "owner-1",
      issue_id: "issue-1",
      issue_status: "in_progress",
      type: "review_requested",
      severity: "action_required",
      title: "Review the migration",
      body: null,
      details: {
        new_reviewer_type: "member",
        new_reviewer_id: "reviewer-1",
      },
      read: false,
      archived: false,
      created_at: "2026-09-03T00:00:00Z",
    } satisfies InboxItem;
    qc.setQueryData<InboxItem[]>(key, [item]);

    patchInboxIssueStatus(
      qc,
      "workspace-1",
      "issue-1",
      "quality-review",
    );

    expect(qc.getQueryData<InboxItem[]>(key)?.[0]).toMatchObject({
      issue_status: "quality-review",
      type: "review_requested",
      details: item.details,
      read: false,
    });
  });
});
