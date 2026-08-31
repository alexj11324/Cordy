import { EventEmitter } from "node:events";
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { runStandaloneWeb } from "./dev-web.mjs";

const authEnv = {
  NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
  CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
  CLERK_SECRET_KEY: "sk_test_fixture",
  CLERK_JWT_KEY: "jwt-fixture",
  CLERK_ISSUER: "https://issuer.example",
  CLERK_AUTHORIZED_PARTIES: "http://localhost:4317",
  PATCHBAY_DEV_AUTH_READY: "1",
};

describe("standalone Web development launcher", () => {
  it("uses the Next 16 Turbopack default with scoped Next server auth", async () => {
    const calls = [];
    const ensureCalls = [];
    const ensureEnv = async (options) => {
      ensureCalls.push(options);
    };
    const spawnImpl = (command, args, options) => {
      calls.push({ command, args, options });
      const child = new EventEmitter();
      queueMicrotask(() => child.emit("close", 0, null));
      return child;
    };

    const env = {
      FRONTEND_PORT: "4317",
      CLERK_SECRET_KEY: "inherited-secret",
      UNRELATED: "kept",
    };
    const status = await runStandaloneWeb({
      repoRoot: "/repo",
      env,
      argv: ["--hostname", "127.0.0.1"],
      ensureEnv,
      bootstrap: async () => ({ authEnv }),
      spawnImpl,
    });

    assert.equal(status, 0);
    assert.deepEqual(ensureCalls, [{ repoRoot: "/repo", env }]);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0].args, [
      "node_modules/next/dist/bin/next",
      "dev",
      "--port",
      "4317",
      "--hostname",
      "127.0.0.1",
    ]);
    assert.equal(calls[0].args.includes("--webpack"), false);
    assert.equal(calls[0].options.cwd, "/repo/apps/web");
    assert.deepEqual(calls[0].options.env, {
      FRONTEND_PORT: "4317",
      UNRELATED: "kept",
      NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
      CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
      CLERK_SECRET_KEY: "sk_test_fixture",
      CLERK_JWT_KEY: "jwt-fixture",
      CLERK_ISSUER: "https://issuer.example",
      CLERK_AUTHORIZED_PARTIES: "http://localhost:4317",
      PATCHBAY_DEV_AUTH_READY: "1",
    });
  });
});
