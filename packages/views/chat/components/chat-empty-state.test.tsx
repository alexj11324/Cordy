import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import type { Agent } from "@patchbay/core/types";
import enChat from "../../locales/en/chat.json";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";

const setInputDraft = vi.hoisted(() => vi.fn());
const storeState = vi.hoisted(() => ({
  activeSessionId: null as string | null,
  setInputDraft,
}));

vi.mock("@patchbay/core/chat", () => ({
  DRAFT_NEW_SESSION: "__new__",
  useChatStore: Object.assign(
    (selector?: (s: typeof storeState) => unknown) =>
      selector ? selector(storeState) : storeState,
    { getState: () => storeState },
  ),
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({
    actorId,
    className,
  }: {
    actorId: string;
    className?: string;
  }) => (
    <div data-testid="agent-avatar" data-actor-id={actorId} className={className} />
  ),
}));

vi.mock("@lobehub/ui/es/Flex/index", () => ({
  Flexbox: ({
    children,
    className,
    style,
    ...rest
  }: React.PropsWithChildren<{
    className?: string;
    style?: React.CSSProperties;
    [key: string]: unknown;
  }>) => {
    const dom: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(rest)) {
      if (
        key.startsWith("data-") ||
        key.startsWith("aria-") ||
        key === "id" ||
        key === "role"
      ) {
        dom[key] = value;
      }
    }
    return (
      <div className={className} style={style} {...dom}>
        {children}
      </div>
    );
  },
}));

import { EmptyState } from "./chat-empty-state";

const TEST_RESOURCES = { en: { chat: enChat } };

const agent = {
  id: "agent-1",
  name: "Lambda",
  description: "Keeps the board moving.",
} as unknown as Agent;

function renderEmpty(ui: React.ReactElement) {
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      {ui}
    </I18nProvider>,
  );
}

describe("Agent empty state (LobeHub AgentHome mapping)", () => {
  beforeEach(() => {
    storeState.activeSessionId = null;
    setInputDraft.mockClear();
  });

  it("aligns on the shared gutter + column", () => {
    const { container } = renderEmpty(<EmptyState agent={null} />);
    const outer = container.firstElementChild as HTMLElement;
    const inner = outer.firstElementChild as HTMLElement;

    for (const cls of CHAT_GUTTER.split(" ")) expect(outer).toHaveClass(cls);
    for (const cls of CHAT_COLUMN.split(" ")) expect(inner).toHaveClass(cls);
  });

  it("shows first-time copy when no agent is bound", () => {
    renderEmpty(<EmptyState agent={null} />);

    expect(screen.getByRole("heading", { name: enChat.empty_state.first_time_title })).toBeInTheDocument();
    expect(screen.getByText(/They know your workspace/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: enChat.starter_prompts.list_open })).not.toBeInTheDocument();
  });

  it("shows the agent name, description, and opening-question chips", () => {
    renderEmpty(<EmptyState agent={agent} />);

    expect(screen.getByTestId("agent-avatar")).toHaveAttribute("data-actor-id", "agent-1");
    expect(screen.getByRole("heading", { name: "Lambda" })).toBeInTheDocument();
    expect(screen.getByText("Keeps the board moving.")).toBeInTheDocument();
    expect(screen.getByText(enChat.empty_state.returning_subtitle)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: enChat.starter_prompts.list_open })).toBeInTheDocument();
  });

  it("fills the composer draft instead of sending", () => {
    renderEmpty(<EmptyState agent={agent} />);

    fireEvent.click(screen.getByRole("button", { name: enChat.starter_prompts.plan_next }));

    expect(setInputDraft).toHaveBeenCalledWith("__new__", enChat.starter_prompts.plan_next);
  });

  it("writes chips into the open session draft when one is selected", () => {
    storeState.activeSessionId = "session-9";
    renderEmpty(<EmptyState agent={agent} />);

    fireEvent.click(screen.getByRole("button", { name: enChat.starter_prompts.summarize_today }));

    expect(setInputDraft).toHaveBeenCalledWith(
      "session-9",
      enChat.starter_prompts.summarize_today,
    );
  });
});
