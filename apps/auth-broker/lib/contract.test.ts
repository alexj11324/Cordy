import { describe, expect, it } from "vitest";
import schema from "../../../contracts/auth-broker/schema.json";
import {
  AUTH_CONTRACT,
  AUTH_CONTRACT_HEADER,
  AUTH_CONTRACT_VERSION,
  authContractResponseHeaders,
} from "./contract";

describe("versioned authentication contract", () => {
  it("freezes canonical origins, authorities, and Desktop protocol", () => {
    expect(AUTH_CONTRACT).toMatchObject({
      name: "patchbay-auth-broker",
      version: 1,
      origins: {
        broker: "https://accounts.aspectlylabs.com",
        product: "https://patchbay.aspectlylabs.com",
        api: "https://api.aspectlylabs.com",
      },
      desktop: {
        callbackUrl: "patchbay://auth/callback",
        queryParameters: ["code", "state"],
        pkceMethod: "S256",
        bearerAllowedInCallback: false,
      },
      authority: {
        identity: "clerk",
        patchbaySession: "rust-api",
        oneTimeGrant: "rust-api",
        brokerPersistence: "none",
      },
    });
  });

  it("publishes the version on every broker response", () => {
    expect(AUTH_CONTRACT_VERSION).toBe(1);
    expect(authContractResponseHeaders()).toMatchObject({
      [AUTH_CONTRACT_HEADER]: "1",
      "cache-control": "no-store",
    });
  });

  it("keeps the schema closed at every authority-bearing object", () => {
    for (const key of ["origins", "broker", "rustApi", "desktop", "authority"] as const) {
      const property = schema.properties[key];
      expect(property.additionalProperties).toBe(false);
      expect(Object.keys(property.properties)).toEqual(
        expect.arrayContaining(property.required),
      );
    }
  });
});
