import { describe, expect, it } from "vitest";
import type { AgentTask, TimelineEntry } from "@patchbay/core/types";
import { buildIssueAgentConversation } from "./issue-agent-conversation";

function task(id: string, overrides: Partial<AgentTask> = {}): AgentTask {
  return {
    id,
    agent_id: "agent-1",
    runtime_id: "runtime-1",
    issue_id: "issue-1",
    status: "completed",
    priority: 0,
    dispatched_at: "2026-08-28T10:00:01Z",
    started_at: "2026-08-28T10:00:02Z",
    completed_at: "2026-08-28T10:00:10Z",
    result: null,
    error: null,
    created_at: "2026-08-28T10:00:00Z",
    ...overrides,
  };
}

function comment(
  id: string,
  content: string,
  createdAt: string,
  overrides: Partial<TimelineEntry> = {},
): TimelineEntry {
  return {
    type: "comment",
    id,
    actor_type: "member",
    actor_id: "member-1",
    content,
    created_at: createdAt,
    ...overrides,
  };
}

describe("buildIssueAgentConversation", () => {
  it("renders many runs for one issue and agent as one ordered chat", () => {
    const tasks = [
      task("task-1", {
        trigger_comment_id: "comment-1",
        delivered_comment_ids: ["comment-1"],
      }),
      task("task-2", {
        status: "running",
        created_at: "2026-08-28T10:01:00Z",
        started_at: "2026-08-28T10:01:02Z",
        completed_at: null,
        trigger_comment_id: "comment-2",
        delivered_comment_ids: ["comment-2"],
      }),
      task("other-agent", { agent_id: "agent-2" }),
    ];
    const timeline = [
      comment(
        "comment-1",
        "[@Worker](mention://agent/agent-1) do stage 1",
        "2026-08-28T09:59:59Z",
      ),
      comment("answer-1", "Stage 1 complete", "2026-08-28T10:00:10Z", {
        actor_type: "agent",
        actor_id: "agent-1",
        source_task_id: "task-1",
      }),
      comment("comment-2", "Continue through stage 3", "2026-08-28T10:00:59Z", {
        actor_type: "agent",
        actor_id: "coordinator-1",
      }),
    ];

    const conversation = buildIssueAgentConversation({
      issueId: "issue-1",
      agentId: "agent-1",
      tasks,
      timeline,
      initialRunPrompt: "Work on this issue.",
    });

    expect(conversation.messages.map((message) => message.id)).toEqual([
      "comment-1",
      "answer-1",
      "comment-2",
    ]);
    expect(conversation.messages[0]?.content).toBe("@Worker do stage 1");
    expect(conversation.messages[1]?.task_id).toBe("task-1");
    expect(conversation.messageActors["comment-2"]).toEqual({
      actorType: "agent",
      actorId: "coordinator-1",
    });
    expect(conversation.pendingTask?.task_id).toBe("task-2");
    expect(conversation.pendingTask?.supports_queue).toBe(true);
  });

  it("keeps queued follow-ups behind the active turn", () => {
    const conversation = buildIssueAgentConversation({
      issueId: "issue-1",
      agentId: "agent-1",
      tasks: [
        task("running", { status: "running", completed_at: null }),
        task("queued", {
          status: "queued",
          created_at: "2026-08-28T10:02:00Z",
          completed_at: null,
          trigger_summary: "Now do stage 2",
        }),
      ],
      timeline: [],
      initialRunPrompt: "Work on this issue.",
    });

    expect(conversation.pendingTask?.task_id).toBe("running");
    expect(conversation.pendingTask?.queued_tasks).toEqual([
      expect.objectContaining({ task_id: "queued", content: "Now do stage 2" }),
    ]);
  });

  it("shows the live Side Chat while its main task continues in parallel", () => {
    const conversation = buildIssueAgentConversation({
      issueId: "issue-1",
      agentId: "agent-1",
      tasks: [
        task("main", { status: "running", completed_at: null }),
        task("side-chat", {
          status: "running",
          created_at: "2026-08-28T10:02:00Z",
          completed_at: null,
          side_chat_parent_task_id: "main",
          side_chat_root_comment_id: "comment-2",
        }),
      ],
      timeline: [],
      initialRunPrompt: "Work on this issue.",
    });

    expect(conversation.pendingTask?.task_id).toBe("side-chat");
    expect(conversation.pendingTask?.queued_tasks).toEqual([
      expect.objectContaining({ task_id: "main", status: "running" }),
    ]);
  });
});
