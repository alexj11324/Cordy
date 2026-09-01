import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import type { Agent } from "@patchbay/core/types";
import enChat from "../../locales/en/chat.json";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";

vi.mock("../../runtimes/components/acp-avatar", () => ({
  AgentAcpAvatar: ({
    agentId,
    shape,
    size,
  }: {
    agentId: string;
    shape?: string;
    size?: number;
  }) => (
    <div
      data-testid="agent-avatar"
      data-actor-id={agentId}
      data-shape={shape}
      data-size={size}
    />
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

describe("Agent empty state (LobeHub AgentInfo mapping)", () => {
  beforeEach(() => {});

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

  it("shows a square runtime logo, the agent name, and the description as the greeting", () => {
    renderEmpty(<EmptyState agent={agent} />);

    const avatar = screen.getByTestId("agent-avatar");
    expect(avatar).toHaveAttribute("data-actor-id", "agent-1");
    expect(avatar).toHaveAttribute("data-shape", "square");
    expect(avatar).toHaveAttribute("data-size", "64");
    expect(screen.getByRole("heading", { name: "Lambda" })).toBeInTheDocument();
    expect(screen.getByText("Keeps the board moving.")).toBeInTheDocument();
    expect(screen.queryByText(enChat.empty_state.returning_subtitle)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: enChat.starter_prompts.list_open })).not.toBeInTheDocument();
  });

  it("falls back to the default greeting when the agent has no description", () => {
    renderEmpty(
      <EmptyState agent={{ ...agent, description: "" } as Agent} />,
    );

    expect(
      screen.getByText(enChat.empty_state.greeting.replace("{{name}}", "Lambda")),
    ).toBeInTheDocument();
  });
});
