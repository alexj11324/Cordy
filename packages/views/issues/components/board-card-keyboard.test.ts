// @vitest-environment node

import { describe, expect, it } from "vitest";
import { shouldStopBoardCardDragKey } from "./board-card-keyboard";

describe("board card link keyboard handling", () => {
  it("keeps Enter on the issue link out of the drag activator", () => {
    expect(shouldStopBoardCardDragKey("Enter")).toBe(true);
    expect(shouldStopBoardCardDragKey(" ")).toBe(false);
  });
});
