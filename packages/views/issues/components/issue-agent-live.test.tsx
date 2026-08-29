// @vitest-environment jsdom

import type { ReactElement } from "react";
import { cleanup, fireEvent, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentTask } from "@patchbay/core/types";
import { renderWithI18n } from "../../test/i18n";

const mockState = vi.hoisted(() => ({
  tasks: [] as AgentTask[],
}));

vi.mock("thinking-orbs", () => ({
  ThinkingOrb: ({ state }: { state?: string }) => (
    <span data-testid="thinking-orb" data-state={state} />
  ),
}));

vi.mock("./issue-agent-conversation-dialog", () => ({
  IssueAgentConversationDialog: ({
    agentId,
    tasks,
  }: {
    agentId: string;
    tasks: AgentTask[];
  }) => {
    const selectedTask = tasks.find((task) => task.agent_id === agentId);
    return (
      <div role="dialog" data-agent-id={agentId} data-task-id={selectedTask?.id}>
        {typeof selectedTask?.result === "object" &&
        selectedTask.result &&
        "output" in selectedTask.result
          ? String((selectedTask.result as { output?: string }).output ?? "")
          : ""}
      </div>
    );
  },
  IssueAgentConversationTrigger: ({ onClick }: { onClick: () => void }) => (
    <button type="button" aria-label="Open conversation" onClick={onClick} />
  ),
  useIssueAgentMessageSend: () => ({
    send: vi.fn(async () => false),
    isSending: false,
  }),
  useIssueAgentTasks: () => mockState.tasks,
}));

vi.mock("./reply-input", () => ({
  ReplyInput: () => (
    <div data-testid="agent-working-reply">Leave a reply...</div>
  ),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) =>
    selector({ user: { id: "user-1" } }),
}));

vi.mock("@tanstack/react-query", async () => {
  const actual =
    await vi.importActual<typeof import("@tanstack/react-query")>(
      "@tanstack/react-query",
    );
  return {
    ...actual,
    useQuery: (opts: { queryKey?: readonly unknown[] }) => {
      if (opts.queryKey?.[0] === "issues" && opts.queryKey?.[1] === "tasks") {
        return { data: mockState.tasks };
      }
      return actual.useQuery(opts as Parameters<typeof actual.useQuery>[0]);
    },
  };
});

vi.mock("@patchbay/core/workspace/hooks", () => ({
  useActorName: () => ({
    getActorName: (_type: string, id: string) =>
      ({
        "agent-research": "Research",
        "agent-coding": "Coding",
      })[id] ?? "Unknown Agent",
  }),
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({ actorId }: { actorId: string }) => (
    <span data-testid="actor-avatar">{actorId}</span>
  ),
}));

import {
  IssueAgentWorkingStatus,
  pickIssueAgentLiveTask,
} from "./issue-agent-live";

const RESEARCH_ID = "aaaaaaaa-1111-4111-8111-111111111111";
const CODING_ID = "bbbbbbbb-2222-4222-8222-222222222222";

function makeTask(overrides: Partial<AgentTask>): AgentTask {
  return {
    id: RESEARCH_ID,
    agent_id: "agent-research",
    runtime_id: "runtime-1",
    issue_id: "issue-1",
    status: "completed",
    priority: 0,
    dispatched_at: "2026-06-08T08:00:00Z",
    started_at: "2026-06-08T08:00:00Z",
    completed_at: "2026-06-08T08:05:00Z",
    result: { output: "Empty states are consistent." },
    error: null,
    created_at: "2026-06-08T08:00:00Z",
    ...overrides,
  };
}

function renderLive(ui: ReactElement) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderWithI18n(
    <QueryClientProvider client={qc}>{ui}</QueryClientProvider>,
  );
}

beforeEach(() => {
  cleanup();
  mockState.tasks = [];
});

describe("pickIssueAgentLiveTask", () => {
  it("prefers a running task over a queued one", () => {
    const queued = makeTask({
      id: RESEARCH_ID,
      status: "queued",
      completed_at: null,
    });
    const running = makeTask({
      id: CODING_ID,
      agent_id: "agent-coding",
      status: "running",
      completed_at: null,
      started_at: "2026-06-08T09:00:00Z",
    });
    expect(pickIssueAgentLiveTask([queued, running])?.id).toBe(CODING_ID);
  });

  it("returns undefined when nothing is live", () => {
    expect(pickIssueAgentLiveTask([makeTask({})])).toBeUndefined();
  });
});

describe("IssueAgentWorkingStatus", () => {
  it("renders the working orb, nested reply, and conversation trigger", () => {
    mockState.tasks = [
      makeTask({
        id: CODING_ID,
        agent_id: "agent-coding",
        status: "running",
        completed_at: null,
        result: null,
      }),
    ];
    renderLive(<IssueAgentWorkingStatus issueId="issue-1" />);
    expect(screen.getByTestId("thinking-orb")).toHaveAttribute(
      "data-state",
      "working",
    );
    expect(screen.getByText("Working...")).toBeInTheDocument();
    expect(screen.getByText("Coding")).toBeInTheDocument();
    expect(screen.getByTestId("agent-working-reply")).toHaveTextContent(
      "Leave a reply...",
    );
    expect(
      screen.getByRole("button", { name: "Open conversation" }),
    ).toBeInTheDocument();
  });

  it("opens this agent's conversation from the header button", () => {
    mockState.tasks = [
      makeTask({}),
      makeTask({
        id: CODING_ID,
        agent_id: "agent-coding",
        status: "running",
        completed_at: null,
        result: null,
      }),
    ];
    renderLive(<IssueAgentWorkingStatus issueId="issue-1" />);
    fireEvent.click(screen.getByRole("button", { name: "Open conversation" }));
    expect(screen.getByRole("dialog")).toHaveAttribute(
      "data-agent-id",
      "agent-coding",
    );
  });

  it("hides itself when no agent is live", () => {
    mockState.tasks = [makeTask({})];
    renderLive(<IssueAgentWorkingStatus issueId="issue-1" />);
    expect(screen.queryByTestId("thinking-orb")).not.toBeInTheDocument();
    expect(screen.queryByTestId("agent-working-reply")).not.toBeInTheDocument();
  });
});
