import { describe, expect, it } from "vitest";
import type { AgentTask } from "@orvilo/core/types";
import { selectIssueAgentConversations } from "./issue-agent-conversations-popover";

function task(
  id: string,
  agentId: string,
  status: AgentTask["status"],
  createdAt: string,
  issueId = "issue-1",
): AgentTask {
  return {
    id,
    agent_id: agentId,
    runtime_id: "runtime-1",
    issue_id: issueId,
    status,
    priority: 0,
    dispatched_at: null,
    started_at: null,
    completed_at: null,
    result: null,
    error: null,
    created_at: createdAt,
  };
}

describe("selectIssueAgentConversations", () => {
  it("lists each Agent once and selects its active continuation", () => {
    const rows = selectIssueAgentConversations(
      [
        task("codex-root", "codex", "completed", "2026-09-09T10:00:00Z"),
        task("codex-steer", "codex", "running", "2026-09-09T11:00:00Z"),
        task("luna-root", "luna", "completed", "2026-09-09T12:00:00Z"),
      ],
      [],
      "issue-1",
    );

    expect(rows.map(({ task: row }) => row.id)).toEqual(["codex-steer", "luna-root"]);
  });

  it("uses the newest task for an idle Agent and excludes other issues", () => {
    const rows = selectIssueAgentConversations(
      [
        task("old", "codex", "failed", "2026-09-09T10:00:00Z"),
        task("new", "codex", "completed", "2026-09-09T12:00:00Z"),
        task("foreign", "luna", "running", "2026-09-09T13:00:00Z", "issue-2"),
      ],
      [],
      "issue-1",
    );

    expect(rows.map(({ task: row }) => row.id)).toEqual(["new"]);
  });
});
