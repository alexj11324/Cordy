// @vitest-environment node
import { describe, expect, it } from "vitest";
import { shouldCloseSettingsOnEscape } from "./settings-escape";

function escapeEvent(
  init: {
    key?: string;
    defaultPrevented?: boolean;
    altKey?: boolean;
    ctrlKey?: boolean;
    metaKey?: boolean;
    isComposing?: boolean;
    target?: EventTarget | null;
  } = {},
): KeyboardEvent {
  return {
    key: "Escape",
    defaultPrevented: false,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    isComposing: false,
    target: null,
    ...init,
  } as KeyboardEvent;
}

describe("shouldCloseSettingsOnEscape", () => {
  it("closes Settings on a bare Escape", () => {
    expect(shouldCloseSettingsOnEscape(escapeEvent())).toBe(true);
  });

  it("ignores other keys", () => {
    expect(shouldCloseSettingsOnEscape(escapeEvent({ key: "Enter" }))).toBe(
      false,
    );
  });

  it("defers when the event was already handled", () => {
    const event = escapeEvent();
    Object.defineProperty(event, "defaultPrevented", { value: true });
    expect(shouldCloseSettingsOnEscape(event)).toBe(false);
  });

  it("defers while an IME is composing", () => {
    expect(
      shouldCloseSettingsOnEscape(escapeEvent({ isComposing: true })),
    ).toBe(false);
  });

  it("ignores modified Escape", () => {
    expect(shouldCloseSettingsOnEscape(escapeEvent({ metaKey: true }))).toBe(
      false,
    );
    expect(shouldCloseSettingsOnEscape(escapeEvent({ altKey: true }))).toBe(
      false,
    );
  });
});
