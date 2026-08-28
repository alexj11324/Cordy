// @vitest-environment node
import { describe, expect, it } from "vitest";

import { isOfficialMarketingHost } from "./public-host";

describe("isOfficialMarketingHost", () => {
  it.each([
    "aspectlylabs.com",
    "www.aspectlylabs.com",
    "ASPECTLYLABS.COM",
    "aspectlylabs.com.",
  ])(
    "recognizes %s as an official marketing host",
    (host) => {
      expect(isOfficialMarketingHost(host)).toBe(true);
    },
  );

  it.each(["app.aspectlylabs.com", "api.aspectlylabs.com", "localhost", "patchbay.test"])(
    "does not treat %s as the public marketing host",
    (host) => {
      expect(isOfficialMarketingHost(host)).toBe(false);
    },
  );
});
