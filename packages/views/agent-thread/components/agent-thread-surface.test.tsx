// @vitest-environment jsdom

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: () => <div data-testid="agent-avatar" />,
}));

vi.mock("../../chat/components/chat-message-list", () => ({
  ChatMessageList: ({ showProcessSteps }: { showProcessSteps?: boolean }) => (
    <div data-testid="messages" data-show-process-steps={String(showProcessSteps)} />
  ),
  ChatMessageSkeleton: () => <div data-testid="messages-loading" />,
}));

vi.mock("../../chat/components/chat-queue", () => ({
  ChatQueue: () => <div data-testid="queue" />,
}));

vi.mock("../../chat/components/chat-input", () => ({
  ChatInput: ({
    disabled,
    isRunning,
    allowSubmitWhileRunning,
    onStop,
  }: {
    disabled?: boolean;
    isRunning?: boolean;
    allowSubmitWhileRunning?: boolean;
    onStop?: () => void;
  }) => (
    <div
      data-testid="chat-input"
      data-disabled={disabled || undefined}
      data-allow-submit-while-running={allowSubmitWhileRunning}
    >
      {isRunning ? <button type="button" onClick={onStop}>Stop task</button> : null}
    </div>
  ),
}));

import { AgentThreadSurface } from "./agent-thread-surface";

describe("AgentThreadSurface", () => {
  it("keeps Stop available when a pre-session task cannot continue yet", () => {
    const onStop = vi.fn();
    const onClose = vi.fn();
    render(
      <AgentThreadSurface
        onClose={onClose}
        collapseLabel="Collapse Agent conversation sidebar"
        agentId="agent-1"
        agentName="Scout"
        title="Scout conversation"
        messages={[]}
        pendingTask={{ task_id: "task-queued", status: "queued" }}
        availability="offline"
        unavailableReason="The provider has not established a session for this run yet."
        allowSubmitWhileRunning
        onSend={vi.fn()}
        onStop={onStop}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent("not established a session");
    expect(screen.getByTestId("chat-input")).toHaveAttribute("data-disabled", "true");
    expect(screen.getByTestId("chat-input")).toHaveAttribute(
      "data-allow-submit-while-running",
      "false",
    );
    fireEvent.click(screen.getByRole("button", { name: "Stop task" }));
    expect(onStop).toHaveBeenCalledOnce();
    const panel = screen.getByRole("complementary", { name: "Scout conversation" });
    expect(panel).toHaveClass("min-w-0", "overflow-hidden");
    fireEvent.click(screen.getByRole("button", { name: "Collapse Agent conversation sidebar" }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("uses compact conversation chrome without tool rows or a composer avatar", () => {
    render(
      <AgentThreadSurface
        onClose={vi.fn()}
        collapseLabel="Back"
        agentId="agent-1"
        agentName="Scout"
        title="Scout conversation"
        description="Long panel-only explanation"
        messages={[]}
        pendingTask={null}
        availability="online"
        onSend={vi.fn()}
        compact
      />,
    );

    expect(screen.queryByText("Long panel-only explanation")).not.toBeInTheDocument();
    expect(screen.getByTestId("messages")).toHaveAttribute(
      "data-show-process-steps",
      "false",
    );
    expect(screen.getAllByTestId("agent-avatar")).toHaveLength(1);
  });
});
