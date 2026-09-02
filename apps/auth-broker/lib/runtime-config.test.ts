import { describe, expect, it } from "vitest";
import { readAuthBrokerRuntimeConfig } from "./runtime-config";
const valid = { PATCHBAY_API_ORIGIN: "https://api.aspectlylabs.com", PATCHBAY_AUTH_BROKER_ORIGIN: "https://accounts.aspectlylabs.com", CLERK_PUBLISHABLE_KEY: "pk_live_example", PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN: "a".repeat(64), PATCHBAY_ORIGIN_AUTH_TOKEN: "b".repeat(64) };
describe("auth broker runtime config", () => {
  it("accepts the production three-domain contract", () => { expect(readAuthBrokerRuntimeConfig(valid)).toEqual({ ok: true, config: { apiOrigin: "https://api.aspectlylabs.com", brokerOrigin: "https://accounts.aspectlylabs.com", clerkPublishableKey: "pk_live_example", goBrokerAuthToken: "a".repeat(64), originAuthToken: "b".repeat(64) } }); });
  it("rejects a malformed broker secret", () => { expect(readAuthBrokerRuntimeConfig({ ...valid, PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN: "short" }).ok).toBe(false); });
  it("rejects origins containing paths", () => { expect(readAuthBrokerRuntimeConfig({ ...valid, PATCHBAY_API_ORIGIN: "https://api.aspectlylabs.com/v1" }).ok).toBe(false); });
  it("never permits localhost as a broker or API origin", () => { expect(readAuthBrokerRuntimeConfig({ ...valid, PATCHBAY_AUTH_BROKER_ORIGIN: "http://localhost:3100" }).ok).toBe(false); });
});
