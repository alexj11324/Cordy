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

    expect(env.PORT).toBe("8080");
    expect(env.FRONTEND_ORIGIN).toBe("http://localhost:3000");
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
    expect(contents).toMatch(/PATCHBAY_TELEGRAM_SECRET_KEY=[A-Za-z0-9+/]{43}=/);
    expect(contents).toMatch(/PATCHBAY_WEIXIN_SECRET_KEY=[A-Za-z0-9+/]{43}=/);
    expect(Number(env.PORT)).toBeGreaterThanOrEqual(18080);
    expect(Number(env.PORT)).toBeLessThanOrEqual(19079);
  });
});
