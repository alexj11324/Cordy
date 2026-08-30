import { describe, expect, it } from "vitest";
import { buildDesktopHandoffQuery } from "./desktop-handoff";

describe("buildDesktopHandoffQuery", () => {
  it("preserves both PKCE binding parameters", () => {
    const query = buildDesktopHandoffQuery(
      new URLSearchParams(
        "platform=desktop&code_challenge=challenge-value&state=opaque-state",
      ),
    );

    expect(query).toBe(
      "platform=desktop&code_challenge=challenge-value&state=opaque-state",
    );
  });

  it("does not copy unrelated callback parameters", () => {
    const query = buildDesktopHandoffQuery(
      new URLSearchParams(
        "platform=desktop&code_challenge=challenge-value&state=opaque-state&token=secret",
      ),
    );

    expect(query).toBe(
      "platform=desktop&code_challenge=challenge-value&state=opaque-state",
    );
  });
});
