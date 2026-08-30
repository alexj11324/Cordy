import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { describe, expect, it, vi } from "vitest";
import enChat from "../../locales/en/chat.json";
import { ChatQueue } from "./chat-queue";

const TEST_RESOURCES = { en: { chat: enChat } };

function renderQueue(headStatus = "running", sendNowDisabled = false) {
  const callbacks = {
    onSendNow: vi.fn<(taskId: string) => Promise<void>>().mockResolvedValue(),
    onEdit: vi.fn<(taskId: string) => Promise<void>>().mockResolvedValue(),
    onRemove: vi.fn<(taskId: string) => Promise<void>>().mockResolvedValue(),
    onClear: vi.fn<() => Promise<void>>().mockResolvedValue(),
  };
  const view = render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <ChatQueue
        headStatus={headStatus}
        sendNowDisabled={sendNowDisabled}
        tasks={[
          {
            task_id: "task-2",
            status: "queued",
            content: "First follow-up",
            created_at: "2026-07-01T00:01:00Z",
          },
          {
            task_id: "task-3",
            status: "queued",
            content: "",
            created_at: "2026-07-01T00:02:00Z",
          },
        ]}
        {...callbacks}
      />
    </I18nProvider>,
  );
  return { ...callbacks, container: view.container };
}

describe("ChatQueue", () => {
  it("renders a LobeHub-style tray flush above the composer", () => {
    const { container } = renderQueue();

    expect(
      screen.getByRole("region", { name: "2 queued messages" }),
    ).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
    expect(screen.getByText("First follow-up")).toBeInTheDocument();
    expect(screen.getByText("Queued message")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Steer" })).toHaveLength(2);
    expect(screen.getAllByLabelText("Edit queued message")).toHaveLength(2);
    expect(screen.getAllByLabelText("Remove queued message")).toHaveLength(2);
    expect(
      screen.getByRole("button", { name: "Clear all" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByLabelText("More queue actions"),
    ).not.toBeInTheDocument();

    const queue = container.querySelector('[data-slot="chat-queue"]');
    expect(queue).toHaveClass(
      "border-b",
      "border-surface-border",
      "bg-surface-raised",
    );
    expect(queue).not.toHaveClass("rounded-t-xl");
    expect(
      container.querySelectorAll('[data-slot="chat-queue-row"]'),
    ).toHaveLength(2);
    expect(
      container.querySelectorAll('[data-slot="chat-queue-item-icon"]'),
    ).toHaveLength(2);
  });

  it("keeps queued messages visible when the owner has no mutation handlers", () => {
    render(
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        <ChatQueue
          tasks={[
            {
              task_id: "task-2",
              status: "deferred",
              content: "Queued while the provider is finishing",
              created_at: "2026-07-01T00:01:00Z",
            },
          ]}
          headStatus="running"
          readOnly
        />
      </I18nProvider>,
    );

    expect(
      screen.getByRole("region", { name: "1 queued message" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Queued while the provider is finishing"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("runs steer, edit, remove, and clear against the selected queue state", async () => {
    const actions = renderQueue();

    fireEvent.click(screen.getAllByRole("button", { name: "Steer" })[1]!);
    await waitFor(() =>
      expect(actions.onSendNow).toHaveBeenCalledWith("task-3"),
    );

    fireEvent.click(screen.getAllByLabelText("Edit queued message")[0]!);
    await waitFor(() => expect(actions.onEdit).toHaveBeenCalledWith("task-2"));

    fireEvent.click(screen.getAllByLabelText("Remove queued message")[1]!);
    await waitFor(() =>
      expect(actions.onRemove).toHaveBeenCalledWith("task-3"),
    );

    fireEvent.click(screen.getByRole("button", { name: "Clear all" }));
    await waitFor(() => expect(actions.onClear).toHaveBeenCalledTimes(1));
  });

  it("disables send-now until the current positional head is claimable", () => {
    const actions = renderQueue("queued");

    const buttons = screen.getAllByRole("button", {
      name: "Steer is available after the current reply starts",
    });
    expect(buttons).toHaveLength(2);
    for (const button of buttons) expect(button).toBeDisabled();
    expect(actions.onSendNow).not.toHaveBeenCalled();
  });

  it("keeps long queues bounded and blocks duplicate actions while one is pending", async () => {
    let finishClear: (() => void) | undefined;
    const actions = renderQueue();
    actions.onClear.mockReturnValue(
      new Promise<void>((resolve) => {
        finishClear = resolve;
      }),
    );

    const scroller = actions.container.querySelector(
      '[data-slot="chat-queue-list"]',
    );
    expect(scroller).toHaveClass("max-h-40");

    const clearTrigger = screen.getByRole("button", { name: "Clear all" });
    fireEvent.click(clearTrigger);
    await waitFor(() => {
      expect(clearTrigger.querySelector(".animate-spin")).toBeInTheDocument();
      for (const button of screen.getAllByRole("button")) {
        expect(button).toBeDisabled();
      }
    });

    finishClear?.();
    await waitFor(() => {
      expect(
        clearTrigger.querySelector(".animate-spin"),
      ).not.toBeInTheDocument();
      for (const button of screen.getAllByRole("button")) {
        expect(button).toBeEnabled();
      }
    });
    expect(actions.onClear).toHaveBeenCalledTimes(1);
  });
});

// PB-6380: steering a queued message dispatches it now, so it has to clear the
// same invoke gate as a fresh send. When the caller has lost permission to run
// the agent, a live-looking Steer button just walks them into a 403.
describe("ChatQueue send-now gating", () => {
  it("blocks Steer when the caller may no longer invoke the agent", async () => {
    const { onSendNow } = renderQueue("running", true);

    const steer = screen.getAllByRole("button", {
      name: "You no longer have permission to run this agent",
    })[0]!;
    expect(steer).toBeDisabled();
    fireEvent.click(steer);
    await waitFor(() => expect(onSendNow).not.toHaveBeenCalled());
  });

  it("leaves Steer available when the head task is dispatchable and permitted", () => {
    renderQueue("running", false);

    expect(
      screen.getAllByRole("button", { name: "Steer" })[0]!,
    ).not.toBeDisabled();
  });
});
