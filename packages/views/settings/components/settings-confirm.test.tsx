import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { useEffect, useRef } from "react";
import { describe, expect, it } from "vitest";

import { renderWithI18n } from "../../test/i18n";
import {
  type SettingsConfirmHandle,
  type SettingsConfirmOptions,
  useSettingsConfirm,
} from "./settings-confirm";

function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/**
 * Stands in for a tab's row action: the trigger is a hook, so it has to be
 * driven from inside a component. The cleanup destroys whatever dialog it
 * opened — `confirmModal` keeps its stack at module level, so an entry left
 * open would render into the next test's tree.
 */
function ConfirmTrigger({
  onConfirm,
  ...options
}: Partial<SettingsConfirmOptions> & { onConfirm: () => Promise<void> }) {
  const confirm = useSettingsConfirm();
  const handle = useRef<SettingsConfirmHandle | null>(null);

  useEffect(() => () => handle.current?.destroy(), []);

  return (
    <button
      type="button"
      onClick={() => {
        handle.current = confirm({
          title: "Confirm title",
          onConfirm,
          ...options,
        });
      }}
    >
      Open confirm
    </button>
  );
}

const DIALOG_TITLE = "Confirm title";

// A closing dialog leaves the DOM through an exit animation, so "is it gone"
// is only meaningful after a window, not at the instant the request settles.
// Every test below reads the same window, which is what makes the negative
// assertions real: the success path demonstrably closes inside it, so a failed
// request that closed the dialog — or an `onConfirm` that returned no promise
// and let `confirmModal` close immediately — would be caught rather than
// observed too early and reported as "still open".
const CLOSE_WINDOW_MS = 1_000;

async function openConfirm(): Promise<HTMLElement> {
  fireEvent.click(await screen.findByRole("button", { name: "Open confirm" }));
  return screen.findByRole("button", { name: "Confirm" });
}

async function closedWithinWindow(): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText(DIALOG_TITLE)).toBeNull(),
      { timeout: CLOSE_WINDOW_MS },
    );
    return true;
  } catch {
    return false;
  }
}

describe("useSettingsConfirm", () => {
  // `confirmModal` falls back to English with no locale behind it, so an
  // untranslated wrapper shows "Cancel"/"OK" to a Chinese user at exactly the
  // moment something is about to be destroyed.
  it("localizes the button labels", async () => {
    renderWithI18n(<ConfirmTrigger onConfirm={async () => {}} />, {
      lobe: true,
      locale: "zh-Hans",
    });

    fireEvent.click(
      await screen.findByRole("button", { name: "Open confirm" }),
    );

    expect(await screen.findByRole("button", { name: "取消" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "确认" })).toBeTruthy();
  });

  it("closes once the request resolves", async () => {
    const request = deferred();
    renderWithI18n(<ConfirmTrigger onConfirm={() => request.promise} />, {
      lobe: true,
    });

    const okButton = await openConfirm();
    await act(async () => {
      fireEvent.click(okButton);
    });
    // The dialog is genuinely waiting on the request, not ignoring it: the
    // library only marks the button busy when `onOk` handed back a promise.
    expect(okButton.getAttribute("aria-busy")).toBe("true");

    await act(async () => {
      request.resolve();
    });

    expect(await closedWithinWindow()).toBe(true);
  });

  // The whole point of the wrapper: `confirmModal` closes as soon as `onOk`
  // returns unless it returns a promise, and keeps the dialog open when that
  // promise rejects. A failed delete must not dismiss the dialog and leave the
  // UI claiming the row is gone.
  it("stays open when the request fails", async () => {
    const request = deferred();
    renderWithI18n(<ConfirmTrigger onConfirm={() => request.promise} />, {
      lobe: true,
    });

    const okButton = await openConfirm();
    await act(async () => {
      fireEvent.click(okButton);
    });
    expect(okButton.getAttribute("aria-busy")).toBe("true");

    await act(async () => {
      request.reject(new Error("server said no"));
    });

    expect(screen.getByText(DIALOG_TITLE)).toBeTruthy();
    expect(okButton.getAttribute("aria-busy")).toBeNull();
    expect(await closedWithinWindow()).toBe(false);
  });

  it("stays open when the callback forgets to return its promise", async () => {
    renderWithI18n(
      <ConfirmTrigger
        onConfirm={
          // TypeScript forbids this shape; a call site that casts, or plain JS,
          // would otherwise close the dialog before the request even started.
          (() => {}) as unknown as () => Promise<void>
        }
      />,
      { lobe: true },
    );

    const okButton = await openConfirm();
    await act(async () => {
      fireEvent.click(okButton);
    });

    expect(screen.getByText(DIALOG_TITLE)).toBeTruthy();
    // No `aria-busy` at any point: the guard fired instead of the library
    // awaiting a promise that was never returned.
    expect(okButton.getAttribute("aria-busy")).toBeNull();
    expect(await closedWithinWindow()).toBe(false);
  });

  // `ModalConfirmConfig` has no destructive field; `okButtonProps` (and inside
  // it `danger`) is the only hook, and it is what gives the confirming button
  // the red treatment the old `AlertDialogAction` had. Compared against the
  // non-destructive tone rather than against a class name, which is hashed.
  it("tones the confirming button destructively unless told otherwise", async () => {
    async function confirmingButtonClass(tone: "default" | "destructive") {
      const { unmount } = renderWithI18n(
        <ConfirmTrigger tone={tone} onConfirm={async () => {}} />,
        { lobe: true },
      );
      const okButton = await openConfirm();
      const className = okButton.className;
      unmount();
      return className;
    }

    expect(await confirmingButtonClass("destructive")).not.toBe(
      await confirmingButtonClass("default"),
    );
  });
});
