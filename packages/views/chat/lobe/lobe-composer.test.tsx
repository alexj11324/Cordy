//
// Mount + wiring tests for the LobeHub-backed composer.
//
// The editor's own behaviour (lexical editing, markdown round-tripping) belongs
// to the library. The submit-shortcut *matrix* — which chords match, the IME
// guards, Shift+Enter — is canonically tested in
// `packages/views/editor/extensions/submit-shortcut.test.ts`; it is not
// re-run through a DOM mount here. What this file owns is the WIRING: that a
// keydown on the rendered editor actually reaches that guard and calls
// `onSubmit`, which is the part a mount can break on its own.

import { render } from "@testing-library/react";
import { fireEvent } from "@testing-library/react";
import {
  configureShortcutPlatform,
  useShortcutStore,
} from "@orvilo/core/shortcuts";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LobeComposer } from "./lobe-composer";
import { LobeThemeBridge } from "../../lobe";

function renderComposer(props: Partial<Parameters<typeof LobeComposer>[0]> = {}) {
  const utils = render(
    <LobeThemeBridge>
      <LobeComposer onSubmit={vi.fn()} {...props} />
    </LobeThemeBridge>,
  );
  return {
    ...utils,
    // Lexical renders a contenteditable host; if the kernel failed to attach,
    // this is what would be missing.
    editor: utils.container.querySelector<HTMLElement>('[contenteditable="true"]'),
  };
}

beforeEach(() => {
  // Pin the platform so `primary` resolves deterministically to Control rather
  // than depending on the machine running the suite.
  configureShortcutPlatform("windows");
});

afterEach(() => {
  useShortcutStore.getState().resetAll();
  configureShortcutPlatform(null);
});

describe("LobeComposer", () => {
  it("mounts an editable region", () => {
    const { editor } = renderComposer();

    expect(editor).toBeTruthy();
  });

  it("renders the action bar slots it is given", () => {
    const { getByRole } = renderComposer({
      leftActions: <button type="button">Attach</button>,
      rightActions: <button type="button">Send now</button>,
    });

    expect(getByRole("button", { name: "Attach" })).toBeTruthy();
    expect(getByRole("button", { name: "Send now" })).toBeTruthy();
  });

  it("submits on the configured send shortcut", () => {
    const onSubmit = vi.fn();
    const { editor } = renderComposer({ onSubmit });

    // `send` defaults to `primary("Enter")`, i.e. Control+Enter under the
    // pinned platform. This is the regression guard for the library trap that
    // makes `onKeyDown` a silent no-op: wiring the wrong prop leaves the
    // composer looking correct and never firing.
    fireEvent.keyDown(editor!, { key: "Enter", ctrlKey: true });

    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("does not submit on a bare Enter", () => {
    const onSubmit = vi.fn();
    const { editor } = renderComposer({ onSubmit });

    fireEvent.keyDown(editor!, { key: "Enter" });

    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("does not submit on the IME Enter that commits a composition", () => {
    const onSubmit = vi.fn();
    const { editor } = renderComposer({ onSubmit });

    // A Chinese/Japanese user pressing Enter to commit a candidate sends
    // `keyCode: 229`. Safari additionally clears `isComposing` on exactly this
    // event, so this is the case a naive `event.isComposing` check misses —
    // and getting it wrong sends a half-typed message.
    fireEvent.keyDown(editor!, { key: "Enter", ctrlKey: true, keyCode: 229 });

    expect(onSubmit).not.toHaveBeenCalled();
  });
});
