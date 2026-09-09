// @vitest-environment node

import { describe, expect, it } from "vitest";
import { classifyDirectoryInviteError } from "./team-directory-invite";

describe("team directory invitation failures", () => {
  it("routes capacity failures to the matching recovery path", () => {
    expect(classifyDirectoryInviteError("seat_capacity_full")).toBe(
      "purchase",
    );
    expect(classifyDirectoryInviteError("seat_capacity_overcommitted")).toBe(
      "overcommitted",
    );
    expect(
      classifyDirectoryInviteError("seat_capacity_unavailable"),
    ).toBe("unavailable");
    expect(
      classifyDirectoryInviteError("seat_capacity_rate_limited"),
    ).toBe("rate_limited");
    expect(classifyDirectoryInviteError("invalid_email")).toBe("unknown");
  });
});
