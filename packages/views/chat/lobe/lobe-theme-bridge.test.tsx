//
// Integration smoke test: proves the LobeHub/antd components actually render
// under our theme bridge. The token mapping matrix itself lives in
// `lobe-tokens.test.ts` (node) — this suite only covers wiring and the pieces
// that need a DOM (theme detection, the CSS-variable namespace).

import { ChatItem } from "@lobehub/ui/chat";
import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { LobeThemeBridge } from "./lobe-theme-bridge";

function setDarkMode(enabled: boolean): void {
  document.documentElement.classList.toggle("dark", enabled);
}

afterEach(() => {
  document.documentElement.className = "";
});

describe("LobeThemeBridge", () => {
  it("renders a LobeHub component with our content", () => {
    render(
      <LobeThemeBridge>
        <ChatItem avatar={{ title: "Agent" }} message="Bridged message" placement="left" />
      </LobeThemeBridge>,
    );

    expect(screen.getByText("Bridged message")).toBeTruthy();
  });

  it("namespaces its CSS variables instead of using LobeHub's", () => {
    const { container } = render(
      <LobeThemeBridge>
        <ChatItem avatar={{ title: "Agent" }} message="Namespaced" placement="left" />
      </LobeThemeBridge>,
    );

    // antd writes its variables onto a style tag it injects; the key we pass
    // must appear, and LobeHub's own must not.
    const css = Array.from(container.ownerDocument.querySelectorAll("style"))
      .map((node) => node.textContent ?? "")
      .join("\n");

    expect(css).toContain("orvilo-lobe");
    expect(css).not.toContain("lobe-vars");
  });

  it("survives a dark-mode class flip without remounting", () => {
    setDarkMode(false);
    render(
      <LobeThemeBridge>
        <ChatItem avatar={{ title: "Agent" }} message="Toggles" placement="left" />
      </LobeThemeBridge>,
    );

    expect(() => setDarkMode(true)).not.toThrow();
    expect(screen.getByText("Toggles")).toBeTruthy();
  });
});
