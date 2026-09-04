import { describe, expect, it } from "vitest";
import robots from "./robots";

describe("robots", () => {
  it("keeps the documentation discoverable without advertising app routes", () => {
    expect(robots()).toEqual({
      rules: {
        userAgent: "*",
        allow: ["/docs", "/docs/"],
        disallow: "/",
      },
      sitemap: "https://patchbay.aspectlylabs.com/docs/sitemap.xml",
    });
  });
});
