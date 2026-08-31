import { describe, expect, it } from "vitest";

import { requiresSourceMatchedCli } from "./cli-resolution-policy";

describe("CLI resolution policy", () => {
  it("requires the bundled source CLI in the complete development environment", () => {
    expect(requiresSourceMatchedCli({ PATCHBAY_REQUIRE_SOURCE_CLI: "1" })).toBe(
      true,
    );
  });

  it("keeps release/packaged resolution behavior when the dev gate is absent", () => {
    expect(requiresSourceMatchedCli({})).toBe(false);
  });
});
