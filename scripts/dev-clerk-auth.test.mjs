import { generateKeyPairSync } from "node:crypto";

import { describe, expect, it, vi } from "vitest";

import {
  bootstrapDevClerkAuth,
  defaultSecretProvider,
  issuerFromPublishableKey,
} from "./dev-clerk-auth.mjs";

const issuer = "https://example.clerk.accounts.dev";
const publishableKey = `pk_test_${Buffer.from("example.clerk.accounts.dev$").toString("base64")}`;
const { publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwtKey = publicKey.export({ type: "spki", format: "pem" }).toString();

describe("secure Clerk development bootstrap", () => {
  it("bounds Google Secret Manager access without exposing command output", async () => {
    const execImpl = vi.fn(async (_command, _args, options) => {
      expect(options.timeout).toBe(10_000);
      throw new Error("provider stderr with secret material");
    });
    await expect(
      defaultSecretProvider({
        project: "general-secrets-store",
        secret: "patchbay-dev-clerk-auth",
        execImpl,
      }),
    ).rejects.not.toThrow(/provider stderr|secret material/);
  });

  it("derives the issuer and loads the complete process-only environment from GSM JSON", async () => {
    const secretProvider = vi.fn(async () =>
      JSON.stringify({
        NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
        CLERK_SECRET_KEY: "sk_test_fixture",
        CLERK_JWT_KEY: jwtKey,
      }),
    );
    const env = { FRONTEND_ORIGIN: "http://localhost:13777" };

    const result = await bootstrapDevClerkAuth({ env, secretProvider });

    expect(secretProvider).toHaveBeenCalledWith({
      project: "general-secrets-store",
      secret: "patchbay-dev-clerk-auth",
    });
    expect(issuerFromPublishableKey(publishableKey)).toBe(issuer);
    expect(result).toMatchObject({
      issuer,
      authorizedParties: "http://localhost:13777",
      source: "gsm",
    });
    expect(result.authEnv).toMatchObject({
      NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
      CLERK_PUBLISHABLE_KEY: publishableKey,
      CLERK_SECRET_KEY: "sk_test_fixture",
      CLERK_JWT_KEY: jwtKey,
      CLERK_ISSUER: issuer,
      CLERK_AUTHORIZED_PARTIES: "http://localhost:13777",
      PATCHBAY_DEV_AUTH_READY: "1",
    });
    expect(env.CLERK_SECRET_KEY).toBeUndefined();
  });

  it("uses a complete injected environment without contacting GSM", async () => {
    const secretProvider = vi.fn();
    const env = {
      FRONTEND_ORIGIN: "http://localhost:3000",
      NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
      CLERK_SECRET_KEY: "sk_test_fixture",
      CLERK_JWT_KEY: jwtKey,
    };

    const result = await bootstrapDevClerkAuth({ env, secretProvider });

    expect(result.source).toBe("environment");
    expect(secretProvider).not.toHaveBeenCalled();
  });

  it("derives a PEM verification key from Clerk JWKS metadata", async () => {
    const jwk = publicKey.export({ format: "jwk" });
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({ keys: [{ ...jwk, use: "sig" }] }),
    }));
    const env = { FRONTEND_ORIGIN: "http://localhost:3000" };

    const result = await bootstrapDevClerkAuth({
      env,
      fetchImpl,
      secretProvider: async () => ({
        CLERK_PUBLISHABLE_KEY: publishableKey,
        CLERK_SECRET_KEY: "sk_test_fixture",
      }),
    });
    expect(fetchImpl).toHaveBeenCalledWith(
      `${issuer}/.well-known/jwks.json`,
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(result.authEnv.CLERK_JWT_KEY).toContain("BEGIN PUBLIC KEY");
  });

  it("fails with remediation without exposing secret payload values", async () => {
    const leakedValue = "sk_test_do-not-log-this";
    const stdout = vi.spyOn(console, "log").mockImplementation(() => {});
    const stderr = vi.spyOn(console, "error").mockImplementation(() => {});
    let message = "";
    try {
      await bootstrapDevClerkAuth({
        env: { FRONTEND_ORIGIN: "http://localhost:3000" },
        secretProvider: async () =>
          JSON.stringify({
            CLERK_PUBLISHABLE_KEY: "invalid-publishable-key",
            CLERK_SECRET_KEY: leakedValue,
            CLERK_JWT_KEY: jwtKey,
          }),
      });
    } catch (error) {
      message = error.message;
    }

    expect(message).toContain("Authenticate gcloud for Secret Manager access");
    expect(message).not.toContain(leakedValue);
    expect(stdout).not.toHaveBeenCalled();
    expect(stderr).not.toHaveBeenCalled();
  });

  it("rejects an issuer that does not match the publishable key", async () => {
    await expect(
      bootstrapDevClerkAuth({
        env: { FRONTEND_ORIGIN: "http://localhost:3000" },
        secretProvider: async () => ({
          CLERK_PUBLISHABLE_KEY: publishableKey,
          CLERK_SECRET_KEY: "sk_test_fixture",
          CLERK_JWT_KEY: jwtKey,
          CLERK_ISSUER: "https://other.clerk.accounts.dev",
        }),
      }),
    ).rejects.toThrow(/does not match.*Authenticate gcloud/i);
  });

  it("rejects live Clerk credentials in development", async () => {
    const livePublishable = `pk_live_${Buffer.from("example.clerk.accounts.dev$").toString("base64")}`;
    await expect(
      bootstrapDevClerkAuth({
        env: { FRONTEND_ORIGIN: "http://localhost:3000" },
        secretProvider: async () => ({
          CLERK_PUBLISHABLE_KEY: livePublishable,
          CLERK_SECRET_KEY: "sk_live_fixture",
          CLERK_JWT_KEY: jwtKey,
        }),
      }),
    ).rejects.toThrow(/test.*key/i);
  });
});
