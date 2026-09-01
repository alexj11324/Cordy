import { describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";

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

import { DesktopChatInput } from "./desktop-chat-input";

describe("DesktopChatInput", () => {
  it("keeps LobeHub header/body/footer slots on the shared chat column", () => {
    const { container } = render(
      <DesktopChatInput
        composerRef={{ current: null }}
        leftActions={<span>left</span>}
        rightActions={<span>right</span>}
        header={<div data-testid="header-slot">queued</div>}
      >
        <textarea data-testid="editor" />
      </DesktopChatInput>,
    );

    const outer = container.firstElementChild as HTMLElement;
    const column = outer.firstElementChild as HTMLElement;
    const surface = container.querySelector("[data-slot='chat-input-surface']");
    const actions = container.querySelector("[data-slot='chat-input-actions']");

    for (const cls of CHAT_GUTTER.split(" ")) expect(outer).toHaveClass(cls);
    for (const cls of CHAT_COLUMN.split(" ")) expect(column).toHaveClass(cls);
    expect(surface).toHaveAttribute("data-lobe-chat-input");
    expect(surface?.contains(container.querySelector("[data-testid='header-slot']"))).toBe(true);
    expect(surface?.contains(container.querySelector("[data-testid='editor']"))).toBe(true);
    expect(actions).toBeInTheDocument();
    expect(actions).toHaveTextContent("leftright");
  });
});
