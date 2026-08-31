import assert from "node:assert/strict";
import { generateKeyPairSync } from "node:crypto";
import { describe, it } from "node:test";

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
    const execImpl = async (_command, _args, options) => {
      assert.equal(options.timeout, 10_000);
      throw new Error("provider stderr with secret material");
    };
    await assert.rejects(
      () =>
        defaultSecretProvider({
          project: "general-secrets-store",
          secret: "patchbay-dev-clerk-auth",
          execImpl,
        }),
      (error) => {
        assert.doesNotMatch(error.message, /provider stderr|secret material/);
        return true;
      },
    );
  });

  it("derives the issuer and loads the complete process-only environment from GSM JSON", async () => {
    const secretProviderCalls = [];
    const secretProvider = async (input) => {
      secretProviderCalls.push(input);
      return JSON.stringify({
        NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
        CLERK_SECRET_KEY: "sk_test_fixture",
        CLERK_JWT_KEY: jwtKey,
      });
    };
    const env = { FRONTEND_ORIGIN: "http://localhost:13777" };

    const result = await bootstrapDevClerkAuth({ env, secretProvider });

    assert.deepEqual(secretProviderCalls, [
      {
        project: "general-secrets-store",
        secret: "patchbay-dev-clerk-auth",
      },
    ]);
    assert.equal(issuerFromPublishableKey(publishableKey), issuer);
    assert.equal(result.issuer, issuer);
    assert.equal(result.authorizedParties, "http://localhost:13777");
    assert.equal(result.source, "gsm");
    assert.deepEqual(result.authEnv, {
      NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
      CLERK_PUBLISHABLE_KEY: publishableKey,
      CLERK_SECRET_KEY: "sk_test_fixture",
      CLERK_JWT_KEY: jwtKey,
      CLERK_ISSUER: issuer,
      CLERK_AUTHORIZED_PARTIES: "http://localhost:13777",
      PATCHBAY_DEV_AUTH_READY: "1",
    });
    assert.equal(env.CLERK_SECRET_KEY, undefined);
  });

  it("uses a complete injected environment without contacting GSM", async () => {
    let secretProviderCalled = false;
    const secretProvider = async () => {
      secretProviderCalled = true;
    };
    const env = {
      FRONTEND_ORIGIN: "http://localhost:3000",
      NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
      CLERK_SECRET_KEY: "sk_test_fixture",
      CLERK_JWT_KEY: jwtKey,
    };

    const result = await bootstrapDevClerkAuth({ env, secretProvider });

    assert.equal(result.source, "environment");
    assert.equal(secretProviderCalled, false);
  });

  it("rejects conflicting publishable-key aliases", async () => {
    await assert.rejects(
      () =>
        bootstrapDevClerkAuth({
          env: {
            FRONTEND_ORIGIN: "http://localhost:3000",
            NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
            CLERK_PUBLISHABLE_KEY: publishableKey.replace(
              "example.clerk.accounts.dev",
              "other.clerk.accounts.dev",
            ),
            CLERK_SECRET_KEY: "sk_test_fixture",
            CLERK_JWT_KEY: jwtKey,
          },
          secretProvider: async () => {
            throw new Error("Secret Manager should not be contacted");
          },
        }),
      /publishable-key.*must match/iu,
    );
  });

  it("rejects partial injected credentials before contacting GSM", async () => {
    let secretProviderCalled = false;
    await assert.rejects(
      () =>
        bootstrapDevClerkAuth({
          env: {
            FRONTEND_ORIGIN: "http://localhost:3000",
            CLERK_SECRET_KEY: "sk_test_stale",
          },
          secretProvider: async () => {
            secretProviderCalled = true;
            return {};
          },
        }),
      /complete set.*partial values/iu,
    );
    assert.equal(secretProviderCalled, false);
  });

  it("derives a PEM verification key from Clerk JWKS metadata", async () => {
    const jwk = publicKey.export({ format: "jwk" });
    const fetchCalls = [];
    const fetchImpl = async (...args) => {
      fetchCalls.push(args);
      return {
        ok: true,
        status: 200,
        json: async () => ({ keys: [{ ...jwk, use: "sig" }] }),
      };
    };
    const env = { FRONTEND_ORIGIN: "http://localhost:3000" };

    const result = await bootstrapDevClerkAuth({
      env,
      fetchImpl,
      secretProvider: async () => ({
        CLERK_PUBLISHABLE_KEY: publishableKey,
        CLERK_SECRET_KEY: "sk_test_fixture",
      }),
    });
    assert.equal(fetchCalls.length, 1);
    assert.equal(fetchCalls[0][0], `${issuer}/.well-known/jwks.json`);
    assert.ok(fetchCalls[0][1].signal instanceof AbortSignal);
    assert.match(result.authEnv.CLERK_JWT_KEY, /BEGIN PUBLIC KEY/u);
  });

  it("fails with remediation without exposing secret payload values", async () => {
    const leakedValue = "sk_test_do-not-log-this";
    const stdout = [];
    const stderr = [];
    const originalLog = console.log;
    const originalError = console.error;
    console.log = (...values) => stdout.push(values);
    console.error = (...values) => stderr.push(values);
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
    } finally {
      console.log = originalLog;
      console.error = originalError;
    }

    assert.match(message, /Authenticate gcloud for Secret Manager access/u);
    assert.doesNotMatch(message, new RegExp(leakedValue, "u"));
    assert.equal(stdout.length, 0);
    assert.equal(stderr.length, 0);
  });

  it("rejects an issuer that does not match the publishable key", async () => {
    await assert.rejects(
      () =>
        bootstrapDevClerkAuth({
          env: { FRONTEND_ORIGIN: "http://localhost:3000" },
          secretProvider: async () => ({
            CLERK_PUBLISHABLE_KEY: publishableKey,
            CLERK_SECRET_KEY: "sk_test_fixture",
            CLERK_JWT_KEY: jwtKey,
            CLERK_ISSUER: "https://other.clerk.accounts.dev",
          }),
        }),
      /does not match.*Authenticate gcloud/iu,
    );
  });

  it("rejects live Clerk credentials in development", async () => {
    const livePublishable = `pk_live_${Buffer.from("example.clerk.accounts.dev$").toString("base64")}`;
    await assert.rejects(
      () =>
        bootstrapDevClerkAuth({
          env: { FRONTEND_ORIGIN: "http://localhost:3000" },
          secretProvider: async () => ({
            CLERK_PUBLISHABLE_KEY: livePublishable,
            CLERK_SECRET_KEY: "sk_live_fixture",
            CLERK_JWT_KEY: jwtKey,
          }),
        }),
      /test.*key/iu,
    );
  });
});
