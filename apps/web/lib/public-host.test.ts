// @vitest-environment node
import { describe, expect, it } from "vitest";

import { isOfficialMarketingHost } from "./public-host";

describe("isOfficialMarketingHost", () => {
  it.each([
    "patchbay.aspectlylabs.com",
    "patchbay.ai",
    "www.patchbay.ai",
    "PATCHBAY.AI",
    "patchbay.ai.",
  ])(
    "recognizes %s as an official marketing host",
    (host) => {
      expect(isOfficialMarketingHost(host)).toBe(true);
    },
  );

  it.each(["app.patchbay.ai", "api.patchbay.ai", "localhost", "patchbay.test"])(
    "does not treat %s as the public marketing host",
    (host) => {
      expect(isOfficialMarketingHost(host)).toBe(false);
    },
  );
});
