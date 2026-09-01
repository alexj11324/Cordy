import { describe, expect, it } from "vitest";
import { isPickerSessionMode, pickerSessionModes } from "./session-modes";

describe("pickerSessionModes", () => {
  it("keeps auto_review and value=auto, preserving the advertised label", () => {
    expect(
      pickerSessionModes([
        { value: "auto", label: "Auto", kind: "auto_review" },
        { value: "auto", label: "Approve for me" },
        { value: "ask", label: "Ask" },
        { value: "plan", label: "Plan", kind: "plan" },
        { value: "read-only", label: "Read only" },
        { value: "bypassPermissions", label: "Full access" },
      ]),
    ).toEqual([{ value: "auto", label: "Auto", kind: "auto_review" }]);
  });

  it("returns an empty list when the protocol advertised no picker modes", () => {
    expect(pickerSessionModes([{ value: "default", label: "Default" }])).toEqual(
      [],
    );
    expect(pickerSessionModes(undefined)).toEqual([]);
  });

  it("does not branch on a provider name — only advertised kind/value", () => {
    expect(
      isPickerSessionMode({
        value: "auto",
        label: "Approve for me",
      }),
    ).toBe(true);
    expect(
      isPickerSessionMode({
        value: "supervised",
        label: "Approve for me",
        kind: "auto_review",
      }),
    ).toBe(true);
    expect(
      isPickerSessionMode({ value: "ask", label: "Ask", kind: "ask" }),
    ).toBe(false);
  });
});
