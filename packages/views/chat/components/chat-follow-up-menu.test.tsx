import { fireEvent, render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { describe, expect, it, vi } from "vitest";
import enChat from "../../locales/en/chat.json";
import { ChatFollowUpMenu } from "./chat-follow-up-menu";

const TEST_RESOURCES = { en: { chat: enChat } };

describe("ChatFollowUpMenu", () => {
  it("offers Wait and Steer without sending until a choice is made", () => {
    const onWait = vi.fn();
    const onSteer = vi.fn();
    const onOpenChange = vi.fn();

    render(
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        <ChatFollowUpMenu
          open
          onOpenChange={onOpenChange}
          onWait={onWait}
          onSteer={onSteer}
        >
          <button type="button">Send</button>
        </ChatFollowUpMenu>
      </I18nProvider>,
    );

    expect(screen.getByText("The agent is still working")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Wait/ }));
    expect(onWait).toHaveBeenCalledTimes(1);
    expect(onSteer).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("steers when the steer row is chosen", () => {
    const onWait = vi.fn();
    const onSteer = vi.fn();

    render(
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        <ChatFollowUpMenu
          open
          onOpenChange={vi.fn()}
          onWait={onWait}
          onSteer={onSteer}
        >
          <button type="button">Send</button>
        </ChatFollowUpMenu>
      </I18nProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: /Steer/ }));
    expect(onSteer).toHaveBeenCalledTimes(1);
    expect(onWait).not.toHaveBeenCalled();
  });
});
