import { describe, expect, it } from "vitest";

import { shouldRenderPreviewSessionChildren } from "./desktop-web-preview-session";

describe("shouldRenderPreviewSessionChildren", () => {
  it("keeps the real Electron renderer mounted outside web preview", () => {
    expect(shouldRenderPreviewSessionChildren(false, null)).toBe(true);
  });

  it("only exposes seeded children for the preview identity", () => {
    expect(shouldRenderPreviewSessionChildren(true, "user-preview")).toBe(true);
    expect(shouldRenderPreviewSessionChildren(true, null)).toBe(false);
    expect(shouldRenderPreviewSessionChildren(true, "real-user")).toBe(false);
  });
});
