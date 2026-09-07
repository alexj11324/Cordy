// @vitest-environment jsdom

import { I18nProvider } from "@patchbay/core/i18n/react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import enChat from "../../locales/en/chat.json";
import { ChatQueue } from "./chat-queue";

const resources = { en: { chat: enChat } };

describe("ChatQueue optional actions", () => {
  it("offers Steer by itself and sends the selected queued task id", async () => {
    const onSendNow = vi.fn();
    render(
      <I18nProvider locale="en" resources={resources}>
        <ChatQueue
          headStatus="running"
          tasks={[{
            task_id: "selected-task",
            status: "queued",
            content: "Use this follow-up next",
            created_at: "2026-09-06T12:00:00Z",
          }]}
          onSendNow={onSendNow}
        />
      </I18nProvider>,
    );

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Steer" }));
    });

    expect(onSendNow).toHaveBeenCalledWith("selected-task");
    expect(screen.queryByRole("button", { name: "Remove queued message" }))
      .not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More queue actions" }))
      .not.toBeInTheDocument();
  });
});
