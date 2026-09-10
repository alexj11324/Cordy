// @vitest-environment jsdom

import { cleanup, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentTask } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";

const mockState = vi.hoisted(() => ({
  tasks: [] as unknown[],
  // Captures the props the chip passes to PopoverTrigger so a test can assert
  // the card is wired to open on hover, not only on click.
  triggerProps: undefined as Record<string, unknown> | undefined,
}));

vi.mock("@orvilo/core/workspace/hooks", () => ({
  useActorName: () => ({
    getActorName: (_type: string, id: string) =>
      ({
        "agent-1": "Walt",
        "agent-2": "Gus",
      })[id] ?? "Unknown Agent",
    getActorInitials: (_type: string, id: string) =>
      ({
        "agent-1": "WA",
        "agent-2": "GU",
      })[id] ?? "UA",
    getActorAvatarUrl: () => null,
  }),
}));

vi.mock("../../agents/components/agent-avatar-stack", () => ({
  AgentAvatarStack: ({ agentIds }: { agentIds: string[] }) => (
    <span data-slot="avatar" data-agent-ids={agentIds.join(",")} />
  ),
}));

vi.mock("@orvilo/ui/components/ui/popover", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    Popover: ({ children }: { children: React.ReactNode }) => (
      <div data-testid="agent-popover">{children}</div>
    ),
    PopoverTrigger: ({
      render,
      children,
      ...props
    }: {
      render: React.ReactElement;
      children: React.ReactNode;
    } & Record<string, unknown>) => {
      mockState.triggerProps = props;
      return React.cloneElement(render, undefined, children);
    },
    PopoverContent: ({ children }: { children: React.ReactNode }) => (
      <div data-testid="agent-popover-content">{children}</div>
    ),
  };
});

vi.mock("./execution-log-section", () => ({
  ActiveTaskRow: ({
    task,
    showConversationAction,
  }: {
    task: AgentTask;
    showConversationAction?: boolean;
  }) => (
    <div
      data-testid="active-task-row"
      data-conversation-action={showConversationAction ? "visible" : "hidden"}
    >
      {task.id}
    </div>
  ),
}));

vi.mock("@tanstack/react-query", async () => {
  const actual =
    await vi.importActual<typeof import("@tanstack/react-query")>(
      "@tanstack/react-query",
    );

  return {
    ...actual,
    useQuery: (opts: { queryKey?: readonly unknown[] }) => {
      // Per-issue task list: issueKeys.tasks(issueId) === ["issues","tasks",id]
      if (opts.queryKey?.[0] === "issues" && opts.queryKey?.[1] === "tasks") {
        return { data: mockState.tasks };
      }
      return actual.useQuery(opts as Parameters<typeof actual.useQuery>[0]);
    },
  };
});

import { IssueAgentHeaderChip } from "./issue-agent-header-chip";

const LIVE_TASK_ID = "4a2e8d1c-7f9b-4e2a-9c1d-123456789abc";

function makeTask(overrides: Partial<AgentTask>): AgentTask {
  return {
    id: LIVE_TASK_ID,
    agent_id: "agent-1",
    runtime_id: "runtime-1",
    issue_id: "issue-1",
    status: "running",
    priority: 0,
    dispatched_at: null,
    started_at: "2026-06-08T08:00:00Z",
    completed_at: null,
    result: null,
    error: null,
    created_at: "2026-06-08T08:00:00Z",
    ...overrides,
  };
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockState.tasks = [];
  mockState.triggerProps = undefined;
});

describe("IssueAgentHeaderChip", () => {
  it("shows the active agent name without event count or elapsed time", () => {
    mockState.tasks = [makeTask({})];

    renderWithI18n(<IssueAgentHeaderChip issueId="issue-1" />);

    expect(
      screen.getByRole("button", { name: "Walt is working" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Walt is working")).toBeInTheDocument();
    expect(screen.queryByText(/events?/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/\d+[smh]/i)).not.toBeInTheDocument();
  });

  it("keeps the header popover card with active task rows", () => {
    mockState.tasks = [makeTask({ id: "task-running" })];

    renderWithI18n(<IssueAgentHeaderChip issueId="issue-1" />);

    expect(screen.getByTestId("agent-popover-content")).toBeInTheDocument();
    expect(screen.getByTestId("active-task-row")).toHaveTextContent(
      "task-running",
    );
    expect(screen.getByTestId("active-task-row")).toHaveAttribute(
      "data-conversation-action",
      "hidden",
    );
  });

  it("opens the activity card on hover, not only on click", () => {
    mockState.tasks = [makeTask({})];

    renderWithI18n(<IssueAgentHeaderChip issueId="issue-1" />);

    // Base UI gates hover-to-open on `openOnHover` on the trigger. Without it

    // The trigger stays a real <button>, so click/keyboard access is retained.
    expect(mockState.triggerProps?.openOnHover).toBe(true);
    expect(
      screen.getByRole("button", { name: "Walt is working" }),
    ).toBeInTheDocument();
  });

  it("uses the concise multi-agent working label", () => {
    mockState.tasks = [
      makeTask({ id: "task-1", agent_id: "agent-1" }),
      makeTask({ id: "task-2", agent_id: "agent-2" }),
    ];

    renderWithI18n(<IssueAgentHeaderChip issueId="issue-1" />);

    expect(
      screen.getByRole("button", { name: "2 agents working" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText("2 agents working")).toHaveLength(2);
    expect(screen.getAllByTestId("active-task-row")).toHaveLength(2);
  });

  it("uses the requested Chinese single-agent copy", () => {
    mockState.tasks = [makeTask({})];

    renderWithI18n(<IssueAgentHeaderChip issueId="issue-1" />, {
      locale: "zh-Hans",
    });

    expect(screen.getByText("Walt 在工作")).toBeInTheDocument();
  });

  it("hides the visible label on mobile while preserving a larger tap target and accessible name", () => {
    mockState.tasks = [makeTask({ status: "queued" })];

    renderWithI18n(<IssueAgentHeaderChip issueId="issue-1" />);

    const trigger = screen.getByRole("button", { name: "Walt is queued" });
    expect(trigger.querySelector("[data-slot='avatar']")).not.toBeNull();
    expect(trigger.className).toContain("h-9 min-w-9");
    expect(trigger.className).toContain("md:h-7 md:min-w-0");
    const label = screen.getByText("Walt is queued");
    expect(label.className).toContain("hidden");
    expect(label.className).toContain("md:inline");
  });

  it("does not render when the issue has only terminal tasks", () => {
    // The list is issue-scoped by the endpoint, so the chip's only job is to
    // ignore terminal statuses (those are the execution log's story).
    mockState.tasks = [
      makeTask({
        id: "task-done",
        status: "completed",
        completed_at: "2026-06-08T08:05:00Z",
      }),
      makeTask({
        id: "task-cancelled",
        status: "cancelled",
        completed_at: "2026-06-08T08:06:00Z",
      }),
    ];

    renderWithI18n(<IssueAgentHeaderChip issueId="issue-1" />);

    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
