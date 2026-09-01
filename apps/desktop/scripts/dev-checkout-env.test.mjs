import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  allocateWorktreeOffset,
  createWorktreeEnvFile,
  ensureDevCheckoutEnv,
  loadDevCheckoutEnv,
  selectDevEnvFile,
  validateGeneratedDevCheckoutEnv,
} from "./dev-checkout-env.mjs";

let sandbox;

afterEach(async () => {
  if (sandbox) await rm(sandbox, { recursive: true, force: true });
  sandbox = undefined;
});

describe("development checkout environment", () => {
  it("loads a linked worktree env for a standalone doctor invocation", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-env-"));
    await writeFile(
      join(sandbox, ".git"),
      "gitdir: /tmp/repo/.git/worktrees/dev\n",
    );
    await writeFile(
      join(sandbox, ".env.worktree"),
      "PORT=18123\nPATCHBAY_TELEGRAM_SECRET_KEY=from-file\n",
    );
    const env = {};

    const result = loadDevCheckoutEnv({ repoRoot: sandbox, env });

    expect(result.envFile).toBe(join(sandbox, ".env.worktree"));
    expect(env.PORT).toBe("18123");
    expect(env.PATCHBAY_TELEGRAM_SECRET_KEY).toBe("from-file");
    expect(env.APP_ENV).toBe("development");
    expect(env.PATCHBAY_PUBLIC_URL).toBe("http://localhost:18123");
  });

  it("uses checkout values consistently and expands their references", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-env-"));
    await mkdir(join(sandbox, ".git"));
    await writeFile(
      join(sandbox, ".env.worktree"),
      "FRONTEND_PORT=3000\nFRONTEND_ORIGIN=http://localhost:${FRONTEND_PORT}\nPORT=8080\n",
    );
    const env = { APP_ENV: "production", PORT: "19090" };

    loadDevCheckoutEnv({ repoRoot: sandbox, env });

    expect(env.APP_ENV).toBe("development");
    expect(env.PORT).toBe("8080");
    expect(env.FRONTEND_ORIGIN).toBe("http://localhost:3000");
    expect(env.LOCAL_UPLOAD_DIR).toBe(join(sandbox, "server/data/uploads"));
  });

  it("keeps Clerk credentials process-only and ignores checkout-file values", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-env-"));
    await mkdir(join(sandbox, ".git"));
    await writeFile(
      join(sandbox, ".env.worktree"),
      "CLERK_SECRET_KEY=must-not-load\nCLERK_JWT_KEY=must-not-load\nPORT=8080\n",
    );
    const env = { CLERK_SECRET_KEY: "process-only" };

    loadDevCheckoutEnv({ repoRoot: sandbox, env });

    expect(env.CLERK_SECRET_KEY).toBe("process-only");
    expect(env.CLERK_JWT_KEY).toBeUndefined();
  });

  it("resolves file references against inherited variables", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-env-"));
    await mkdir(join(sandbox, ".git"));
    await writeFile(
      join(sandbox, ".env.worktree"),
      "PORT=${DEV_PORT}\nDATABASE_URL=postgres://${DEV_DATABASE_AUTH}@localhost:${DEV_DATABASE_PORT}/patchbay\n",
    );
    const env = {
      DEV_PORT: "19090",
      DEV_DATABASE_AUTH: "patchbay:runtime-password",
      DEV_DATABASE_PORT: "5544",
    };

    loadDevCheckoutEnv({ repoRoot: sandbox, env });

    expect(env.PORT).toBe("19090");
    expect(env.DATABASE_URL).toBe(
      "postgres://patchbay:runtime-password@localhost:5544/patchbay",
    );
  });

  it("generates an isolated worktree env and local integration keys", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "Patchbay Feature "));
    await writeFile(
      join(sandbox, ".git"),
      "gitdir: /tmp/repo/.git/worktrees/dev\n",
    );
    const env = {};
    const reservationRegistryPath = join(sandbox, "reservations.json");
    await createWorktreeEnvFile({
      repoRoot: sandbox,
      allocateOffset: async () => 607,
      reservationRegistryPath,
    });

    const { envFile } = await ensureDevCheckoutEnv({
      repoRoot: sandbox,
      env,
      log: { log() {} },
      reservationRegistryPath,
    });
    const contents = await readFile(envFile, "utf8");

    expect(selectDevEnvFile({ repoRoot: sandbox, env: {} })).toBe(
      join(sandbox, ".env.worktree"),
    );
    expect(contents).toMatch(
      /POSTGRES_DB=patchbay_patchbay_feature_[a-z0-9_]+_[a-f0-9]{8}_607/,
    );
    expect(contents).toContain("PATCHBAY_DEV_ENV_SCHEMA=3");
    expect(contents).toContain("PATCHBAY_DEV_CHECKOUT_OFFSET=607");
    expect(contents).toContain("APP_ENV=development");
    for (const key of [
      "PATCHBAY_LARK_SECRET_KEY",
      "PATCHBAY_SLACK_SECRET_KEY",
      "PATCHBAY_DINGTALK_SECRET_KEY",
      "PATCHBAY_WECOM_SECRET_KEY",
      "PATCHBAY_TELEGRAM_SECRET_KEY",
      "PATCHBAY_WEIXIN_SECRET_KEY",
    ]) {
      expect(contents).toMatch(new RegExp(`${key}=[A-Za-z0-9+/]{43}=`));
    }
    expect(Number(env.PORT)).toBeGreaterThanOrEqual(18080);
    expect(Number(env.PORT)).toBeLessThanOrEqual(19079);
  });

  it("isolates an independent clone instead of copying generic .env defaults", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-independent-clone-"));
    await mkdir(join(sandbox, ".git"));
    await writeFile(
      join(sandbox, ".env.example"),
      "POSTGRES_DB=patchbay\nPORT=8080\nFRONTEND_PORT=3000\n",
    );
    const env = {};
    const reservationRegistryPath = join(sandbox, "reservations.json");

    const { envFile } = await ensureDevCheckoutEnv({
      repoRoot: sandbox,
      env,
      log: { log() {} },
      allocateOffset: async () => 411,
      reservationRegistryPath,
    });
    const contents = await readFile(envFile, "utf8");

    expect(envFile).toBe(join(sandbox, ".env.worktree"));
    expect(contents).not.toContain("POSTGRES_DB=patchbay\n");
    expect(contents).not.toContain("PORT=8080\n");
    expect(contents).not.toContain("FRONTEND_PORT=3000\n");
    expect(env.POSTGRES_DB).toMatch(/^patchbay_patchbay_independent_clone_/);
    expect(Number(env.PORT)).toBeGreaterThanOrEqual(18080);
    expect(Number(env.FRONTEND_PORT)).toBeGreaterThanOrEqual(13000);
  });

  it("honors an explicit dev env file without reusing .env implicitly", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-explicit-dev-env-"));
    await mkdir(join(sandbox, ".git"));
    await writeFile(join(sandbox, ".env"), "PORT=19090\n");
    const env = { PATCHBAY_DEV_ENV_FILE: ".env" };

    await ensureDevCheckoutEnv({
      repoRoot: sandbox,
      env,
      log: { log() {} },
    });

    expect(env.ENV_FILE).toBe(join(sandbox, ".env"));
    expect(env.PORT).toBe("19090");
  });

  it("ignores Make's generic ENV_FILE when no dev override was requested", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-make-dev-env-"));

    expect(
      selectDevEnvFile({
        repoRoot: sandbox,
        env: { ENV_FILE: ".env" },
      }),
    ).toBe(join(sandbox, ".env.worktree"));
  });

  it("rejects a stale generated env before it can reuse default ports or DB", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-stale-dev-env-"));
    await writeFile(
      join(sandbox, ".env.worktree"),
      "POSTGRES_DB=patchbay\nPORT=8080\nFRONTEND_PORT=3000\n",
    );

    await expect(
      ensureDevCheckoutEnv({
        repoRoot: sandbox,
        env: {},
        log: { log() {} },
        allocateOffset: async () => 17,
      }),
    ).rejects.toThrow(
      /not a current isolated checkout environment.*FORCE=1 make worktree-env/,
    );
  });

  it("accepts the current generated env contract", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-current-dev-env-"));
    const reservationRegistryPath = join(sandbox, "reservations.json");
    await createWorktreeEnvFile({
      repoRoot: sandbox,
      allocateOffset: async () => 17,
      reservationRegistryPath,
    });
    const values = Object.fromEntries(
      (await readFile(join(sandbox, ".env.worktree"), "utf8"))
        .split("\n")
        .filter((line) => line.includes("="))
        .map((line) => line.split("=")),
    );

    expect(
      validateGeneratedDevCheckoutEnv({ repoRoot: sandbox, values }),
    ).toBeNull();
  });

  it("reports local bind permission errors instead of misclassifying them as occupied ports", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-port-permission-"));
    const error = Object.assign(new Error("operation not permitted"), {
      code: "EPERM",
    });

    await expect(
      allocateWorktreeOffset(sandbox, {
        portCheck: async () => {
          throw error;
        },
      }),
    ).rejects.toThrow(/operating system denied binding.*restricted sandbox/);
  });

  it("fails closed when the shared port reservation registry is corrupt", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-corrupt-port-registry-"));
    const registryPath = join(sandbox, "reservations.json");
    await writeFile(registryPath, "{not-json");

    await expect(
      allocateWorktreeOffset(sandbox, {
        reservationRegistryPath: registryPath,
        portCheck: async () => true,
      }),
    ).rejects.toThrow(/port reservation registry .* is corrupt/);
  });

  it("fails closed when a reservation has a valid JSON shape but invalid ports", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-invalid-port-registry-"));
    const registryPath = join(sandbox, "reservations.json");
    await writeFile(
      registryPath,
      JSON.stringify({
        schema: 1,
        reservations: [
          {
            repoRoot: sandbox,
            envFile: join(sandbox, ".env.worktree"),
            offset: 17,
            ports: [],
          },
        ],
      }),
    );

    await expect(
      allocateWorktreeOffset(sandbox, {
        reservationRegistryPath: registryPath,
        portCheck: async () => true,
      }),
    ).rejects.toThrow(/invalid port tuple/);
  });

  it("serializes concurrent port reservations before writing env files", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-env-lock-"));
    const firstRoot = join(sandbox, "first");
    const secondRoot = join(sandbox, "second");
    await mkdir(firstRoot);
    await mkdir(secondRoot);
    const lockPath = join(sandbox, "port-reservation.lock");
    const reservationRegistryPath = join(sandbox, "port-reservations.json");
    let activeAllocations = 0;
    let maximumConcurrentAllocations = 0;
    let nextOffset = 17;
    const allocateOffset = async () => {
      activeAllocations += 1;
      maximumConcurrentAllocations = Math.max(
        maximumConcurrentAllocations,
        activeAllocations,
      );
      await new Promise((resolve) => setTimeout(resolve, 25));
      activeAllocations -= 1;
      return nextOffset++;
    };

    await Promise.all([
      createWorktreeEnvFile({
        repoRoot: firstRoot,
        envFile: join(firstRoot, ".env.worktree"),
        allocateOffset,
        reservationLockPath: lockPath,
        reservationRegistryPath,
      }),
      createWorktreeEnvFile({
        repoRoot: secondRoot,
        envFile: join(secondRoot, ".env.worktree"),
        allocateOffset,
        reservationLockPath: lockPath,
        reservationRegistryPath,
      }),
    ]);

    expect(maximumConcurrentAllocations).toBe(1);
    expect(await readFile(join(firstRoot, ".env.worktree"), "utf8")).toContain(
      "PATCHBAY_DEV_CHECKOUT_OFFSET=17",
    );
    expect(await readFile(join(secondRoot, ".env.worktree"), "utf8")).toContain(
      "PATCHBAY_DEV_CHECKOUT_OFFSET=18",
    );
  });

  it("reserves ports across independent clones and identities their DB/app names", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-independent-collision-"));
    const firstRoot = join(sandbox, "first", "patchbay");
    const secondRoot = join(sandbox, "second", "patchbay");
    await mkdir(firstRoot, { recursive: true });
    await mkdir(secondRoot, { recursive: true });
    const lockPath = join(sandbox, "port-reservation.lock");
    const reservationRegistryPath = join(sandbox, "port-reservations.json");

    await createWorktreeEnvFile({
      repoRoot: firstRoot,
      envFile: join(firstRoot, ".env.worktree"),
      allocateOffset: async () => 411,
      reservationLockPath: lockPath,
      reservationRegistryPath,
    });
    await createWorktreeEnvFile({
      repoRoot: secondRoot,
      envFile: join(secondRoot, ".env.worktree"),
      allocateOffset: (root, options) =>
        allocateWorktreeOffset(root, {
          ...options,
          portCheck: async () => true,
        }),
      reservationLockPath: lockPath,
      reservationRegistryPath,
    });

    const first = Object.fromEntries(
      (await readFile(join(firstRoot, ".env.worktree"), "utf8"))
        .split("\n")
        .filter((line) => line.includes("="))
        .map((line) => line.split("=")),
    );
    const second = Object.fromEntries(
      (await readFile(join(secondRoot, ".env.worktree"), "utf8"))
        .split("\n")
        .filter((line) => line.includes("="))
        .map((line) => line.split("=")),
    );

    expect(second.PATCHBAY_DEV_CHECKOUT_OFFSET).not.toBe(
      first.PATCHBAY_DEV_CHECKOUT_OFFSET,
    );
    expect(second.POSTGRES_DB).not.toBe(first.POSTGRES_DB);
    expect(second.DESKTOP_APP_SUFFIX).not.toBe(first.DESKTOP_APP_SUFFIX);
    expect(await readFile(reservationRegistryPath, "utf8")).toContain(
      secondRoot,
    );
  });
});
