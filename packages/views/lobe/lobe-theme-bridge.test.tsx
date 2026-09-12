//
// Integration smoke test: proves the LobeHub/antd components actually render
// under our theme bridge. The token mapping matrix itself lives in
// `lobe-tokens.test.ts` (node) — this suite only covers wiring and the pieces
// that need a DOM: theme detection, the CSS-variable namespace, and the
// providers every animated component resolves out of context.

import { Button, Form, confirmModal } from "@lobehub/ui/base-ui";
import { ChatItem } from "@lobehub/ui/chat";
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { LobeThemeBridge } from "./lobe-theme-bridge";

function setDarkMode(enabled: boolean): void {
  document.documentElement.classList.toggle("dark", enabled);
}

/**
 * `confirmModal` pushes onto a module-level stack that outlives a test, so an
 * entry left open here would render into the next test's tree.
 */
let openConfirm: { destroy: () => void } | null = null;

function openConfirmation(title: string): void {
  act(() => {
    openConfirm = confirmModal({ title, content: `${title} — body` });
  });
}

afterEach(() => {
  act(() => openConfirm?.destroy());
  openConfirm = null;
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

  // `ChatItem` above never reaches `useMotionComponent`, so it cannot prove
  // the bridge is complete. These are the components the settings surface is
  // built from, and they resolve their animation component from context: with
  // no `MotionProvider` in the tree they throw before rendering anything.
  it("renders motion-driven base-ui components", () => {
    render(
      <LobeThemeBridge>
        <Button>Bridged button</Button>
        <Form.SubmitFooter />
      </LobeThemeBridge>,
    );

    expect(screen.getByText("Bridged button")).toBeTruthy();
  });

  // `confirmModal` is the settings page's replacement for every `AlertDialog`,
  // and it is a no-op unless something in the tree renders `ModalHost` — the
  // library never mounts one itself. Removing `<LobeModalHost />` from the
  // bridge has to fail here, or this suite certifies a dead feature.
  it("renders a dialog for confirmModal", () => {
    render(
      <LobeThemeBridge>
        <div>Bridge anchor</div>
      </LobeThemeBridge>,
    );

    openConfirmation("Disconnect GitHub?");

    expect(screen.getByText("Disconnect GitHub?")).toBeTruthy();
    expect(screen.getByText("Disconnect GitHub? — body")).toBeTruthy();
  });

  // Two bridges in one tree is the normal case, not an edge case: the chat
  // message list and the feedback dialog each mount one, and the dialog opens
  // over the chat page. Both hosts read the same module-level stack, so a
  // second host would paint a second copy of every dialog — and in a dev build
  // it throws before that (`base-ui`'s modal host is a dev-mode singleton).
  it("renders one dialog when two bridges are mounted", () => {
    render(
      <>
        <LobeThemeBridge>
          <div>First bridge</div>
        </LobeThemeBridge>
        <LobeThemeBridge>
          <div>Second bridge</div>
        </LobeThemeBridge>
      </>,
    );

    openConfirmation("Delete workspace?");

    expect(screen.getAllByText("Delete workspace?")).toHaveLength(1);
  });
});
