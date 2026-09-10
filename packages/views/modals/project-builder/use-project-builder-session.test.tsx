// @vitest-environment jsdom

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, StrictMode, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Agent, Attachment } from "@orvilo/core/types";
import type {
  ProjectBuilderCatalogs,
  ProjectBuilderDraft,
} from "./project-builder-draft";

const h = vi.hoisted(() => ({
  createChatSession: vi.fn(),
  sendChatMessage: vi.fn(),
  cancelTaskById: vi.fn(),
  deleteChatSession: vi.fn(),
  listChatMessages: vi.fn(),
  getPendingChatTask: vi.fn(),
}));

vi.mock("@orvilo/core/api", () => ({
  ApiError: class ApiError extends Error {
    status = 0;
  },
  api: h,
}));
vi.mock("../../common/use-app-foreground", () => ({
  useAppForeground: () => true,
}));
vi.mock("../../chat/components/use-chat-draft-restore", () => ({
  useChatDraftRestore: () => ({
    restoreDraftRequest: null,
    handleRestoreDraftApplied: vi.fn(),
  }),
}));
vi.mock("../../i18n", () => ({
  useT: () => ({ t: () => "project builder error" }),
}));

import { useProjectBuilderSession } from "./use-project-builder-session";

const draft: ProjectBuilderDraft = {
  title: "",
  summary: "",
  description: "",
  icon: null,
  status: "planned",
  priority: "none",
  lead_type: null,
  lead_id: null,
  start_date: null,
  due_date: null,
  member_ids: [],
  label_ids: [],
  dependency_ids: [],
};

const catalogs: ProjectBuilderCatalogs = {
  members: [],
  agents: [],
  labels: [],
  projects: [],
};

const agent = {
  id: "agent-1",
  runtime_id: "runtime-1",
  runtime_bound: true,
  name: "Planner",
} as Agent;

function harness(selectedAgent: Agent | null = agent, strict = false) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(
      QueryClientProvider,
      { client: queryClient },
      strict ? createElement(StrictMode, null, children) : children,
    );
  return renderHook(
    () => useProjectBuilderSession({ agent: selectedAgent, draft, catalogs }),
    { wrapper },
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  h.listChatMessages.mockResolvedValue([]);
  h.getPendingChatTask.mockResolvedValue(null);
  h.cancelTaskById.mockResolvedValue({});
  h.deleteChatSession.mockResolvedValue(undefined);
  h.createChatSession.mockResolvedValue({ id: "session-1" });
  h.sendChatMessage.mockResolvedValue({
    message_id: "message-1",
    task_id: "task-1",
    created_at: "2026-09-09T00:00:00Z",
  });
});

afterEach(cleanup);

describe("useProjectBuilderSession lifecycle", () => {
  it("creates a normal chat for the selected agent on the first send", async () => {
    const attachment = { id: "attachment-1" } as Attachment;
    const hook = harness();
    const commitInput = vi.fn();

    await act(async () => {
      await expect(
        hook.result.current.send(
          "Use this screenshot",
          [attachment.id],
          [attachment],
          commitInput,
        ),
      ).resolves.toBe(true);
    });

    expect(h.createChatSession).toHaveBeenCalledTimes(1);
    expect(h.createChatSession).toHaveBeenCalledWith({
      agent_id: "agent-1",
      title: "Create project",
    });
    expect(h.sendChatMessage).toHaveBeenCalledWith(
      "session-1",
      expect.stringContaining('"project_builder_instructions"'),
      [attachment.id],
    );
    expect(commitInput).toHaveBeenCalledTimes(1);
    expect(hook.result.current.sessionId).toBe("session-1");
  });

  it("reuses the created session for follow-up sends", async () => {
    const hook = harness();

    await act(async () => {
      await hook.result.current.send("First request");
      await hook.result.current.send("Follow up");
    });

    expect(h.createChatSession).toHaveBeenCalledTimes(1);
    expect(h.sendChatMessage).toHaveBeenCalledTimes(2);
    expect(h.sendChatMessage.mock.calls[1]?.[0]).toBe("session-1");
  });

  it("deletes the ephemeral session when the panel unmounts", async () => {
    const hook = harness();
    await act(async () => {
      await hook.result.current.send("Create a launch plan");
    });

    hook.unmount();
    await waitFor(() =>
      expect(h.deleteChatSession).toHaveBeenCalledWith("session-1"),
    );
  });

  it("does not delete a session during StrictMode effect replay", async () => {
    const hook = harness(agent, true);

    await act(async () => {
      await hook.result.current.send("Create a plan");
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(h.deleteChatSession).not.toHaveBeenCalled();

    hook.unmount();
    await waitFor(() =>
      expect(h.deleteChatSession).toHaveBeenCalledTimes(1),
    );
  });

  it("deletes a session whose lazy creation resolves after unmount", async () => {
    let resolveCreate!: (value: { id: string }) => void;
    h.createChatSession.mockReturnValueOnce(
      new Promise<{ id: string }>((resolve) => {
        resolveCreate = resolve;
      }),
    );
    const hook = harness();
    let sendPromise: Promise<boolean>;
    act(() => {
      sendPromise = hook.result.current.send("Create a plan");
    });

    hook.unmount();
    resolveCreate({ id: "orphan-session" });
    await expect(sendPromise!).resolves.toBe(false);
    await waitFor(() =>
      expect(h.deleteChatSession).toHaveBeenCalledWith("orphan-session"),
    );
  });

  it("does not create a session when no agent is selected", async () => {
    const hook = harness(null);

    await act(async () => {
      await expect(hook.result.current.send("Create a plan")).resolves.toBe(false);
    });

    expect(h.createChatSession).not.toHaveBeenCalled();
    expect(h.sendChatMessage).not.toHaveBeenCalled();
  });
});
