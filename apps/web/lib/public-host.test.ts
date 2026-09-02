// @vitest-environment node
import { describe, expect, it } from "vitest";

import { isOfficialMarketingHost } from "./public-host";

describe("isOfficialMarketingHost", () => {
  it.each([
    "patchbay.aspectlylabs.com",
    "PATCHBAY.ASPECTLYLABS.COM",
    "patchbay.aspectlylabs.com.",
  ])(
    "recognizes %s as an official marketing host",
    (host) => {
      expect(isOfficialMarketingHost(host)).toBe(true);
    },
  );

  it.each([
    "api.aspectlylabs.com",
    "www.example.invalid",
    "localhost",
    "patchbay.test",
  ])(
    "does not treat %s as the public marketing host",
    (host) => {
      expect(isOfficialMarketingHost(host)).toBe(false);
    },
  );
});
