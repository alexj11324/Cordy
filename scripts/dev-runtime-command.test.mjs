import { EventEmitter } from "node:events";

import { describe, expect, it, vi } from "vitest";

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
    const env = { ...clerkEnv, FRONTEND_ORIGIN: "http://localhost:13000", KEEP: "yes" };
    let preparedEnv;
    let authInput;
    const spawnImpl = vi.fn(() => completedChild());

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

    expect(authInput).toMatchObject(clerkEnv);
    expect(preparedEnv).toEqual({
      FRONTEND_ORIGIN: "http://localhost:13000",
      KEEP: "yes",
    });
    expect(spawnImpl.mock.calls[0][2].env).toMatchObject({
      KEEP: "yes",
      CLERK_SECRET_KEY: "sk_test_inherited",
      CLERK_JWT_KEY: "inherited-jwt",
    });
  });

  it("never adds Clerk fields to non-backend runtime commands", async () => {
    const spawnImpl = vi.fn(() => completedChild());
    const bootstrapAuth = vi.fn();

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

    expect(bootstrapAuth).not.toHaveBeenCalled();
    expect(spawnImpl.mock.calls[0][2].env).toEqual({ KEEP: "yes" });
  });
});
