import { EventEmitter } from "node:events";
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { runDevRuntimeCommand } from "./dev-runtime-command.mjs";

const clerkEnv = {
  NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: "pk_test_inherited",
  CLERK_PUBLISHABLE_KEY: "pk_test_inherited",
  CLERK_SECRET_KEY: "sk_test_inherited",
  CLERK_JWT_KEY: "inherited-jwt",
  CLERK_ISSUER: "https://inherited.example",
  CLERK_AUTHORIZED_PARTIES: "http://localhost:3000",
  PATCHBAY_DEV_AUTH_READY: "1",
};

function completedChild() {
  const child = new EventEmitter();
  queueMicrotask(() => child.emit("close", 0, null));
  return child;
}

describe("development runtime command boundaries", () => {
  it("removes Clerk fields before runtime preparation and restores scoped auth only for the backend", async () => {
    const env = {
      ...clerkEnv,
      FRONTEND_ORIGIN: "http://localhost:13000",
      KEEP: "yes",
    };
    let preparedEnv;
    let authInput;
    const spawnCalls = [];
    const spawnImpl = (...args) => {
      spawnCalls.push(args);
      return completedChild();
    };

    await runDevRuntimeCommand({
      componentId: "backend",
      repoRoot: "/fixture/repo",
      env,
      loadCheckoutEnv: () => {},
      prepareRuntime: async ({ env: receivedEnv }) => {
        preparedEnv = receivedEnv;
      },
      bootstrapAuth: async ({ env: receivedEnv }) => {
        authInput = receivedEnv;
        return { authEnv: clerkEnv };
      },
      listComponents: () => [
        { id: "backend", destinationBinary: "/fixture/patchbay-server" },
      ],
      spawnImpl,
    });

    assert.deepEqual(authInput, env);
    assert.deepEqual(preparedEnv, {
      FRONTEND_ORIGIN: "http://localhost:13000",
      KEEP: "yes",
    });
    assert.equal(spawnCalls[0][2].env.KEEP, "yes");
    assert.equal(
      spawnCalls[0][2].env.CLERK_SECRET_KEY,
      "sk_test_inherited",
    );
    assert.equal(spawnCalls[0][2].env.CLERK_JWT_KEY, "inherited-jwt");
  });

  it("never adds Clerk fields to non-backend runtime commands", async () => {
    const spawnCalls = [];
    const spawnImpl = (...args) => {
      spawnCalls.push(args);
      return completedChild();
    };
    let bootstrapCalls = 0;
    const bootstrapAuth = () => {
      bootstrapCalls += 1;
    };

    await runDevRuntimeCommand({
      componentId: "migrations",
      repoRoot: "/fixture/repo",
      env: { ...clerkEnv, KEEP: "yes" },
      loadCheckoutEnv: () => {},
      prepareRuntime: async () => {},
      bootstrapAuth,
      listComponents: () => [
        { id: "migrations", destinationBinary: "/fixture/patchbay-migrate" },
      ],
      spawnImpl,
    });

    assert.equal(bootstrapCalls, 0);
    assert.deepEqual(spawnCalls[0][2].env, { KEEP: "yes" });
  });

  it("turns Make's explicit ENV_FILE into the runtime loader override", async () => {
    const loadCalls = [];
    const spawnImpl = () => completedChild();

    await runDevRuntimeCommand({
      componentId: "migrations",
      repoRoot: "/fixture/repo",
      env: { ENV_FILE: "/fixture/repo/.env" },
      loadCheckoutEnv: ({ env: loadedEnv }) => loadCalls.push(loadedEnv),
      prepareRuntime: async () => {},
      listComponents: () => [
        { id: "migrations", destinationBinary: "/fixture/patchbay-migrate" },
      ],
      spawnImpl,
    });

    assert.equal(loadCalls[0].PATCHBAY_DEV_ENV_FILE, "/fixture/repo/.env");
  });
});
