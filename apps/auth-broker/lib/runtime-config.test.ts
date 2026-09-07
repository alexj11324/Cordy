import { describe, expect, it } from "vitest";
import { readAuthBrokerRuntimeConfig } from "./runtime-config";
import { AUTH_CONTRACT } from "./contract";
const valid = { ORVILO_API_ORIGIN: "https://api.aspectlylabs.com", ORVILO_AUTH_BROKER_ORIGIN: "https://accounts.aspectlylabs.com", CLERK_PUBLISHABLE_KEY: "pk_live_example", ORVILO_DESKTOP_BROKER_AUTH_TOKEN: "a".repeat(64), ORVILO_ORIGIN_AUTH_TOKEN: "b".repeat(64) };
describe("auth broker runtime config", () => {
  it("accepts the production three-domain contract", () => { expect(readAuthBrokerRuntimeConfig(valid)).toEqual({ ok: true, config: { apiOrigin: "https://api.aspectlylabs.com", brokerOrigin: "https://accounts.aspectlylabs.com", productOrigin: AUTH_CONTRACT.origins.product, clerkPublishableKey: "pk_live_example", goBrokerAuthToken: "a".repeat(64), originAuthToken: "b".repeat(64) } }); });
  it("reads the product origin at runtime and rejects unsafe origins", () => {
    const result = readAuthBrokerRuntimeConfig({ ...valid, ORVILO_PRODUCT_ORIGIN: "https://staging.aspectlylabs.com" });
    expect(result.ok && result.config.productOrigin).toBe("https://staging.aspectlylabs.com");
    for (const value of ["http://staging.aspectlylabs.com", "https://staging.aspectlylabs.com/path", "https://user@staging.aspectlylabs.com"]) {
      expect(readAuthBrokerRuntimeConfig({ ...valid, ORVILO_PRODUCT_ORIGIN: value }).ok).toBe(false);
    }
  });
  it("rejects a malformed broker secret", () => { expect(readAuthBrokerRuntimeConfig({ ...valid, ORVILO_DESKTOP_BROKER_AUTH_TOKEN: "short" }).ok).toBe(false); });
  it("rejects origins containing paths", () => { expect(readAuthBrokerRuntimeConfig({ ...valid, ORVILO_API_ORIGIN: "https://api.aspectlylabs.com/v1" }).ok).toBe(false); });
  it("never permits localhost as a broker or API origin", () => { expect(readAuthBrokerRuntimeConfig({ ...valid, ORVILO_AUTH_BROKER_ORIGIN: "http://localhost:3100" }).ok).toBe(false); });
  it("requires a non-production product origin when the broker origin is not production", () => {
    const staging = {
      ...valid,
      ORVILO_API_ORIGIN: "https://api.staging.aspectlylabs.com",
      ORVILO_AUTH_BROKER_ORIGIN: "https://accounts.staging.aspectlylabs.com",
    };
    expect(readAuthBrokerRuntimeConfig(staging).ok).toBe(false);
    expect(readAuthBrokerRuntimeConfig({
      ...staging,
      ORVILO_PRODUCT_ORIGIN: AUTH_CONTRACT.origins.product,
    }).ok).toBe(false);
    expect(readAuthBrokerRuntimeConfig({
      ...staging,
      ORVILO_PRODUCT_ORIGIN: "https://staging.aspectlylabs.com",
    })).toEqual({
      ok: true,
      config: {
        apiOrigin: "https://api.staging.aspectlylabs.com",
        brokerOrigin: "https://accounts.staging.aspectlylabs.com",
        productOrigin: "https://staging.aspectlylabs.com",
        clerkPublishableKey: "pk_live_example",
        goBrokerAuthToken: "a".repeat(64),
        originAuthToken: "b".repeat(64),
      },
    });
  });
});
