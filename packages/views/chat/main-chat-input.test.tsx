import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MainChatInput } from "./main-chat-input";

const captured = vi.hoisted(() => ({
  engine: undefined as string | undefined,
}));

vi.mock("./components/chat-input", () => ({
  ChatInput: (props: { editorEngine?: string }) => {
    captured.engine = props.editorEngine;
    return <div data-testid="shared-chat-input" />;
  },
}));

describe("MainChatInput", () => {
  it("opts the Agent composer into LobeHub Lexical", () => {
    render(<MainChatInput onSend={vi.fn()} />);
    expect(screen.getByTestId("shared-chat-input")).toBeInTheDocument();
    expect(captured.engine).toBe("lexical");
  });
});
