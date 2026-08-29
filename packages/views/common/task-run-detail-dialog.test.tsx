// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@patchbay/core/i18n/react";
import type { AgentTask } from "@patchbay/core/types";
import enCommon from "../locales/en/common.json";
import enAgents from "../locales/en/agents.json";
import { TaskRunDetailButton } from "./task-run-detail-dialog";

const listTaskMessages = vi.hoisted(() => vi.fn());

vi.mock("@patchbay/core/api", () => ({
  api: { listTaskMessages },
  clientErrorMessage: () => "",
}));

const resources = { en: { common: enCommon, agents: enAgents } };

function task(overrides: Partial<AgentTask> = {}): AgentTask {
  return {
    id: "task-1",
    agent_id: "agent-1",
    runtime_id: "runtime-1",
    issue_id: "",
    status: "completed",
    priority: 0,
    dispatched_at: null,
    started_at: "2026-08-29T12:00:00.000Z",
    completed_at: "2026-08-29T12:00:02.000Z",
    result: null,
    error: null,
    created_at: "2026-08-29T12:00:00.000Z",
    ...overrides,
  };
}

function renderButton(run: AgentTask = task()) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <I18nProvider locale="en" resources={resources}>
      <QueryClientProvider client={queryClient}>
        <TaskRunDetailButton
          task={run}
          agentName="Mika"
          statusLabel="Succeeded"
        />
      </QueryClientProvider>
    </I18nProvider>,
  );
}

describe("TaskRunDetailButton", () => {
  beforeEach(() => {
    listTaskMessages.mockReset();
  });

  it("opens the real read-only task message path for an unlinked run", async () => {
    listTaskMessages.mockResolvedValue([
      {
        task_id: "task-1",
        issue_id: "",
        seq: 1,
        type: "text",
        content: "Inspected the repository",
      },
    ]);

    renderButton();
    fireEvent.click(screen.getByTestId("task-run-detail-trigger"));

    const dialog = await screen.findByTestId("task-run-detail-dialog");
    expect(dialog).toBeInTheDocument();
    expect(dialog).toHaveClass(
      "max-h-[calc(100vh-2rem)]",
      "grid-rows-[auto_minmax(0,1fr)]",
    );
    expect(await screen.findByText("Inspected the repository")).toBeInTheDocument();
    expect(listTaskMessages).toHaveBeenCalledWith("task-1");
  });

  it("keeps a no-message run inspectable without inventing a transcript", async () => {
    listTaskMessages.mockResolvedValue([]);

    renderButton(task({ id: "task-empty" }));
    fireEvent.click(screen.getByTestId("task-run-detail-trigger"));

    expect(
      await screen.findByText("No execution messages were recorded for this run."),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("task-run-detail-events")).not.toBeInTheDocument();
  });
});
