import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  backendUrlFromEnv,
  inspectDevEnvironment,
  integrationKeyStatus,
} from "./dev-environment-doctor.mjs";
import { rustSourceFingerprint } from "./dev-cli-cache.mjs";

let sandbox;

afterEach(async () => {
  vi.restoreAllMocks();
  if (sandbox) await rm(sandbox, { recursive: true, force: true });
  sandbox = undefined;
});

function secretKey(byte = 1) {
  return Buffer.alloc(32, byte).toString("base64");
}

async function fixtureRepo() {
  sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-doctor-"));
  execFileSync("git", ["init", "-q"], { cwd: sandbox });
  await mkdir(join(sandbox, "server-rs"), { recursive: true });
  await writeFile(join(sandbox, "server-rs", "Cargo.toml"), "[workspace]\n");
  execFileSync("git", ["add", "server-rs/Cargo.toml"], { cwd: sandbox });
  const fingerprint = rustSourceFingerprint(sandbox);
  const binary = join(
    sandbox,
    "apps",
    "desktop",
    "resources",
    "bin",
    "patchbay",
  );
  await mkdir(join(binary, ".."), { recursive: true });
  await writeFile(binary, "explicit fixture; never executed");
  await writeFile(
    `${binary}.dev-manifest.json`,
    JSON.stringify({
      sourceFingerprint: fingerprint,
      rustTarget: "aarch64-apple-darwin",
      profile: "dev",
      sha256: createHash("sha256")
        .update("explicit fixture; never executed")
        .digest("hex"),
      buildVariables: { version: "dev-fixture" },
    }),
  );
  return sandbox;
}

describe("complete Desktop development doctor", () => {
  it("never treats an empty or malformed integration key as configured", () => {
    expect(
      integrationKeyStatus({
        PATCHBAY_TELEGRAM_SECRET_KEY: "",
        PATCHBAY_WEIXIN_SECRET_KEY: "not-base64",
      }),
    ).toEqual({
      PATCHBAY_TELEGRAM_SECRET_KEY: false,
      PATCHBAY_WEIXIN_SECRET_KEY: false,
    });
  });

  it("uses the worktree backend endpoint provided by the complete launcher", () => {
    expect(backendUrlFromEnv({ VITE_API_URL: "http://127.0.0.1:18123/" })).toBe(
      "http://127.0.0.1:18123",
    );
  });

  it("validates CLI version, DB-backed readiness, runtime probing and integrations", async () => {
    const repoRoot = await fixtureRepo();
    const execImpl = vi.fn(async (_binary, args) =>
      args[0] === "version"
        ? { stdout: JSON.stringify({ version: "dev-fixture" }) }
        : {
            stdout: JSON.stringify({
              probe_result: "success",
              runtime_count: 2,
              provider_summary: { claude: 1, codex: 1 },
            }),
          },
    );
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({ status: "ready" }),
    }));
    const env = {
      VITE_API_URL: "http://127.0.0.1:18123",
      PATCHBAY_TELEGRAM_SECRET_KEY: secretKey(1),
      PATCHBAY_WEIXIN_SECRET_KEY: secretKey(2),
    };

    const report = await inspectDevEnvironment({
      repoRoot,
      env,
      platform: "darwin",
      arch: "arm64",
      execImpl,
      fetchImpl,
    });

    expect(report.ok).toBe(true);
    expect(report.checks.map(({ id, ok }) => [id, ok])).toEqual([
      ["cli", true],
      ["backend", true],
      ["agents", true],
      ["integrations", true],
    ]);
    expect(execImpl.mock.calls[1][2].env.PATCHBAY_TASK_CONFIG_ROOT).toContain(
      "patchbay-dev-doctor-",
    );
  });

  it("fails explicitly when backend readiness and integration config are missing", async () => {
    const repoRoot = await fixtureRepo();
    const execImpl = vi.fn(async (_binary, args) =>
      args[0] === "version"
        ? { stdout: JSON.stringify({ version: "dev-fixture" }) }
        : {
            stdout: JSON.stringify({
              probe_result: "success",
              runtime_count: 0,
              provider_summary: {},
            }),
          },
    );

    const report = await inspectDevEnvironment({
      repoRoot,
      env: { VITE_API_URL: "http://127.0.0.1:18123" },
      platform: "darwin",
      arch: "arm64",
      execImpl,
      fetchImpl: async () => ({ ok: false, status: 503 }),
    });

    expect(report.ok).toBe(false);
    expect(report.checks.find(({ id }) => id === "backend")?.message).toContain(
      "HTTP 503",
    );
    expect(
      report.checks.find(({ id }) => id === "integrations")?.message,
    ).toContain("PATCHBAY_TELEGRAM_SECRET_KEY");
  });
});
