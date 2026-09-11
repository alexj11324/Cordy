//
// Wiring test for the LobeHub message row: that each prop reaches the
// component that renders it. `ChatItem`'s own behaviour (hover reveal, resize
// adaptation) belongs to the library and is not re-asserted here.

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  LobeMessageItem,
  toChatItemPlacement,
  type LobeMessageItemProps,
} from "./lobe-message-item";
import { LobeThemeBridge } from "../../lobe";

function renderItem(props: LobeMessageItemProps) {
  return render(
    <LobeThemeBridge>
      <LobeMessageItem {...props} />
    </LobeThemeBridge>,
  );
}

describe("LobeMessageItem", () => {
  it("renders the bubble content", () => {
    renderItem({ children: "Agent reply", role: "assistant" });

    expect(screen.getByText("Agent reply")).toBeTruthy();
  });

  it("renders the action row", () => {
    renderItem({
      actions: <button type="button">Copy</button>,
      children: "Agent reply",
      role: "assistant",
    });

    expect(screen.getByRole("button", { name: "Copy" })).toBeTruthy();
  });

  it("renders below-message content", () => {
    renderItem({
      belowMessage: <div>Replied in 38s</div>,
      children: "Agent reply",
      role: "assistant",
    });

    expect(screen.getByText("Replied in 38s")).toBeTruthy();
  });

  it("titles the user row by default", () => {
    renderItem({ children: "hi", role: "user" });

    expect(screen.getByText("You")).toBeTruthy();
  });

  it("titles the agent row by default", () => {
    renderItem({ children: "hi", role: "assistant" });

    expect(screen.getByText("Agent")).toBeTruthy();
  });

  it("prefers a supplied avatar title over the role default", () => {
    renderItem({ avatar: { title: "Researcher" }, children: "hi", role: "assistant" });

    expect(screen.getByText("Researcher")).toBeTruthy();
    expect(screen.queryByText("Agent")).toBeNull();
  });

});

// The placement matrix is a pure mapping, so it is asserted here rather than
// inferred from markup: rendering the theme bridge twice costs seconds under
// jsdom, and antd's generated class names are not a stable contract.
describe("toChatItemPlacement", () => {
  it("sends user messages right and agent messages left", () => {
    expect(toChatItemPlacement("user")).toBe("right");
    expect(toChatItemPlacement("assistant")).toBe("left");
  });
});
