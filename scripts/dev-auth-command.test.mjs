import { EventEmitter } from "node:events";
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { runAuthenticatedDevCommand } from "./dev-auth-command.mjs";

const authEnv = {
  NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
  CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
  CLERK_SECRET_KEY: "sk_test_fixture",
  CLERK_JWT_KEY: "jwt-fixture",
  CLERK_ISSUER: "https://issuer.example",
  CLERK_AUTHORIZED_PARTIES: "http://localhost:3000",
  PATCHBAY_DEV_AUTH_READY: "1",
};

function spawnRecorder() {
  const calls = [];
  const spawnImpl = (command, args, options) => {
    calls.push({ command, args, options });
    const child = new EventEmitter();
    child.kill = () => {};
    queueMicrotask(() => child.emit("close", 0, null));
    return child;
  };
  return { calls, spawnImpl };
}

describe("scoped authenticated development commands", () => {
  it("gives Web public and server-only Clerk fields without inherited values", async () => {
    const { calls, spawnImpl } = spawnRecorder();
    await runAuthenticatedDevCommand({
      scope: "web",
      command: "next",
      env: { CLERK_SECRET_KEY: "inherited", UNRELATED: "kept" },
      bootstrap: async () => ({ authEnv }),
      spawnImpl,
    });

    assert.deepEqual(calls[0].options.env, {
      UNRELATED: "kept",
      NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
      CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
      CLERK_SECRET_KEY: "sk_test_fixture",
      CLERK_JWT_KEY: "jwt-fixture",
      CLERK_ISSUER: "https://issuer.example",
      CLERK_AUTHORIZED_PARTIES: "http://localhost:3000",
      PATCHBAY_DEV_AUTH_READY: "1",
    });
  });

  it("gives backend the verification fields without retaining inherited values", async () => {
    const { calls, spawnImpl } = spawnRecorder();
    await runAuthenticatedDevCommand({
      scope: "backend",
      command: "patchbay-server",
      env: { CLERK_SECRET_KEY: "inherited", CLERK_JWT_KEY: "inherited" },
      bootstrap: async () => ({ authEnv }),
      spawnImpl,
    });

    assert.equal(calls[0].options.env.CLERK_SECRET_KEY, "sk_test_fixture");
    assert.equal(calls[0].options.env.CLERK_JWT_KEY, "jwt-fixture");
  });
});
