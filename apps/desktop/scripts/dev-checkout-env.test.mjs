import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  ensureDevCheckoutEnv,
  loadDevCheckoutEnv,
  selectDevEnvFile,
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
      join(sandbox, ".env"),
      "FRONTEND_PORT=3000\nFRONTEND_ORIGIN=http://localhost:${FRONTEND_PORT}\nPORT=8080\n",
    );
    const env = { PORT: "19090" };

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
      join(sandbox, ".env"),
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
      join(sandbox, ".env"),
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

    const { envFile } = await ensureDevCheckoutEnv({
      repoRoot: sandbox,
      env,
      log: { log() {} },
    });
    const contents = await readFile(envFile, "utf8");

    expect(selectDevEnvFile({ repoRoot: sandbox, env: {} })).toBe(
      join(sandbox, ".env.worktree"),
    );
    expect(contents).toContain("POSTGRES_DB=patchbay_patchbay_feature_");
    expect(contents).toContain("APP_ENV=development");
    for (const key of [
      "PATCHBAY_LARK_SECRET_KEY",
      "PATCHBAY_SLACK_SECRET_KEY",
      "PATCHBAY_DINGTALK_SECRET_KEY",
      "PATCHBAY_WECOM_SECRET_KEY",
      "PATCHBAY_TELEGRAM_SECRET_KEY",
      "PATCHBAY_WEIXIN_SECRET_KEY",
    ]) {
      expect(contents).toMatch(
        new RegExp(`${key}=[A-Za-z0-9+/]{43}=`),
      );
    }
    expect(Number(env.PORT)).toBeGreaterThanOrEqual(18080);
    expect(Number(env.PORT)).toBeLessThanOrEqual(19079);
  });
});
