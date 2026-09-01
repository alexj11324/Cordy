import { execFile, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  accountsUrlFromEnv,
  backendUrlFromEnv,
  inspectDevEnvironment,
  integrationKeyStatus,
  loadDoctorEnvironment,
  nodeRuntimeCheck,
  printDevEnvironmentReport,
  summarizeDevEnvironmentChecks,
} from "./dev-environment-doctor.mjs";
import { rustSourceFingerprint } from "./dev-cli-cache.mjs";
import { INTEGRATION_SECRET_KEYS } from "../../../scripts/ensure-dev-integration-secrets.mjs";

let sandbox;
const execFileAsync = promisify(execFile);

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
  await writeFile(
    binary,
    `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === "version") {
  process.stdout.write(JSON.stringify({ version: "dev-fixture" }));
} else {
  process.stdout.write(JSON.stringify({
    probe_result: "success",
    runtime_count: 2,
    provider_summary: { claude: 1, codex: 1 },
  }));
}
`,
  );
  const fixtureContents = await readFile(binary, "utf8");
  await writeFile(
    `${binary}.dev-manifest.json`,
    JSON.stringify({
      sourceFingerprint: fingerprint,
      rustTarget: "aarch64-apple-darwin",
      profile: "dev",
      sha256: createHash("sha256").update(fixtureContents).digest("hex"),
      buildVariables: { version: "dev-fixture" },
    }),
  );
  return sandbox;
}

describe("complete Desktop development doctor", () => {
  it("reports the pinned Node runtime instead of silently accepting a different major", () => {
    expect(nodeRuntimeCheck("22.14.0")).toEqual({
      id: "node",
      ok: true,
      message: "Node.js 22.14.0 (required major 22)",
    });
    expect(nodeRuntimeCheck("26.7.0")).toMatchObject({
      id: "node",
      ok: false,
      fix: expect.stringContaining("Node.js 22"),
    });
  });

  it("never treats an empty or malformed integration key as configured", () => {
    const env = Object.fromEntries(
      INTEGRATION_SECRET_KEYS.map((key, index) => [key, secretKey(index + 1)]),
    );
    env.PATCHBAY_TELEGRAM_SECRET_KEY = "";
    env.PATCHBAY_WEIXIN_SECRET_KEY = "not-base64";
    expect(integrationKeyStatus(env)).toEqual(
      Object.fromEntries(
        INTEGRATION_SECRET_KEYS.map((key) => [
          key,
          key !== "PATCHBAY_TELEGRAM_SECRET_KEY" &&
            key !== "PATCHBAY_WEIXIN_SECRET_KEY",
        ]),
      ),
    );
  });

  it("keeps runtime readiness separate from pending provider acceptance", () => {
    const checks = [
      { id: "backend", ok: true },
      {
        id: "agent-roundtrip",
        ok: true,
        status: "pending",
        electronToManagedDaemonVerified: false,
        managedDaemonToBackendVerified: false,
        agentExecutionVerified: false,
        message:
          "Electron → managed daemon → backend → agent execution round-trip is not verified by this pre-launch doctor",
      },
      {
        id: "integrations",
        ok: true,
        status: "pending",
        providerAccountsVerified: false,
        messageRoundTripsVerified: false,
        message: "Telegram and WeChat message round-trips are not verified",
      },
    ];
    const report = {
      checks,
      ...summarizeDevEnvironmentChecks(checks),
    };
    const log = {
      log: vi.fn(),
      warn: vi.fn(),
      error: vi.fn(),
    };

    expect(report).toMatchObject({ ok: true, acceptanceOk: false });
    printDevEnvironmentReport(report, log);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining("agent execution round-trip is not verified"),
    );
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining("Telegram and WeChat message round-trips"),
    );
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining("acceptance checks remain pending"),
    );
    expect(log.error).not.toHaveBeenCalled();
  });

  it("uses the worktree backend endpoint provided by the complete launcher", () => {
    expect(backendUrlFromEnv({ VITE_API_URL: "http://127.0.0.1:18123/" })).toBe(
      "http://127.0.0.1:18123",
    );
  });

  it("resolves the hosted accounts broker separately from the API", () => {
    expect(
      accountsUrlFromEnv({
        VITE_ACCOUNTS_URL: "https://accounts.aspectlylabs.com/",
      }),
    ).toBe("https://accounts.aspectlylabs.com");
  });

  it("preserves a launcher's hosted profile when the checkout env is reloaded", async () => {
    const repoRoot = await fixtureRepo();
    const envFile = join(repoRoot, ".env.worktree");
    await writeFile(
      envFile,
      "PORT=18123\nVITE_API_URL=http://127.0.0.1:18123\nVITE_ACCOUNTS_URL=http://localhost:13123\n",
    );

    const env = loadDoctorEnvironment({
      repoRoot,
      processEnv: {
        PATCHBAY_DEV_MODE: "hosted",
        PATCHBAY_APP_URL: "https://patchbay.aspectlylabs.com",
        VITE_API_URL: "https://api.aspectlylabs.com",
        VITE_WS_URL: "wss://api.aspectlylabs.com/ws",
        VITE_APP_URL: "https://patchbay.aspectlylabs.com",
        VITE_ACCOUNTS_URL: "https://accounts.aspectlylabs.com",
      },
    });

    expect(env).toMatchObject({
      PATCHBAY_DEV_MODE: "hosted",
      PATCHBAY_APP_URL: "https://patchbay.aspectlylabs.com",
      VITE_API_URL: "https://api.aspectlylabs.com",
      VITE_WS_URL: "wss://api.aspectlylabs.com/ws",
      VITE_APP_URL: "https://patchbay.aspectlylabs.com",
      VITE_ACCOUNTS_URL: "https://accounts.aspectlylabs.com",
    });
  });

  it("can explicitly inspect the hosted profile from a standalone doctor", async () => {
    const repoRoot = await fixtureRepo();
    const envFile = join(repoRoot, ".env.worktree");
    await writeFile(
      envFile,
      "PORT=18123\nVITE_API_URL=http://127.0.0.1:18123\nVITE_ACCOUNTS_URL=http://localhost:13123\n",
    );

    const env = loadDoctorEnvironment({
      repoRoot,
      mode: "hosted",
      processEnv: {},
    });

    expect(env).toMatchObject({
      PATCHBAY_DEV_MODE: "hosted",
      VITE_API_URL: "https://api.aspectlylabs.com",
      VITE_WS_URL: "wss://api.aspectlylabs.com/ws",
      VITE_APP_URL: "https://patchbay.aspectlylabs.com",
      VITE_ACCOUNTS_URL: "https://accounts.aspectlylabs.com",
    });
  });

  it("checks the hosted accounts broker before opening Electron", async () => {
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
    const fetchImpl = vi.fn(async (url) => ({
      ok: true,
      status: 200,
      json: async () =>
        url.includes("accounts") ? { status: "ready" } : { status: "ready" },
    }));

    const report = await inspectDevEnvironment({
      repoRoot,
      env: {
        PATCHBAY_DEV_MODE: "hosted",
        VITE_API_URL: "https://api.aspectlylabs.com",
        VITE_ACCOUNTS_URL: "https://accounts.aspectlylabs.com",
        ...Object.fromEntries(
          INTEGRATION_SECRET_KEYS.map((key, index) => [
            key,
            secretKey(index + 1),
          ]),
        ),
      },
      platform: "darwin",
      arch: "arm64",
      execImpl,
      fetchImpl,
      cacheRoot: join(repoRoot, "cache"),
      toolchainIdentityImpl: () => null,
    });

    expect(report.ok).toBe(true);
    expect(report.acceptanceOk).toBe(false);
    expect(report.checks.map(({ id, ok }) => [id, ok])).toEqual([
      ["cache", true],
      ["cli", true],
      ["backend", true],
      ["accounts", true],
      ["agents", true],
      ["agent-roundtrip", true],
      ["integrations", true],
    ]);
    expect(report.checks.find(({ id }) => id === "integrations")).toMatchObject(
      {
        status: "pending",
        providerAccountsVerified: false,
        messageRoundTripsVerified: false,
        providers: {
          telegram: {
            encryptionKeyConfigured: true,
            credentialKind: "BotFather token",
            providerCredentialStatus: "not_verified",
            messageRoundTripStatus: "not_verified",
          },
          weixin: {
            encryptionKeyConfigured: true,
            credentialKind: "iLink QR authorization",
            providerCredentialStatus: "not_verified",
            messageRoundTripStatus: "not_verified",
          },
        },
        message: expect.stringContaining(
          "neither message round-trip has been run",
        ),
      },
    );
    expect(
      report.checks.find(({ id }) => id === "agent-roundtrip"),
    ).toMatchObject({
      status: "pending",
      preflightReady: false,
      localAgentAvailable: false,
      electronToManagedDaemonVerified: false,
      managedDaemonToBackendVerified: false,
      agentExecutionVerified: false,
    });
    expect(fetchImpl).toHaveBeenCalledWith(
      "https://accounts.aspectlylabs.com/readyz",
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
  }, 15_000);

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
      ...Object.fromEntries(
        INTEGRATION_SECRET_KEYS.map((key, index) => [
          key,
          secretKey(index + 1),
        ]),
      ),
    };

    const report = await inspectDevEnvironment({
      repoRoot,
      env,
      platform: "darwin",
      arch: "arm64",
      execImpl,
      fetchImpl,
      cacheRoot: join(repoRoot, "cache"),
      toolchainIdentityImpl: () => null,
    });

    expect(report.ok).toBe(true);
    expect(report.acceptanceOk).toBe(false);
    expect(report.checks.map(({ id, ok }) => [id, ok])).toEqual([
      ["cache", true],
      ["cli", true],
      ["backend", true],
      ["agents", true],
      ["agent-roundtrip", true],
      ["integrations", true],
    ]);
    expect(report.checks.find(({ id }) => id === "integrations")).toMatchObject(
      {
        status: "pending",
      },
    );
    expect(
      report.checks.find(({ id }) => id === "agent-roundtrip"),
    ).toMatchObject({
      status: "pending",
      preflightReady: true,
      localAgentAvailable: true,
      electronToManagedDaemonVerified: false,
      managedDaemonToBackendVerified: false,
      agentExecutionVerified: false,
    });
    expect(execImpl.mock.calls[1][2].env).not.toHaveProperty(
      "PATCHBAY_TASK_CONFIG_ROOT",
    );
    expect(execImpl.mock.calls[0][0]).toBe(execImpl.mock.calls[1][0]);
    expect(execImpl.mock.calls[0][0]).not.toBe(report.binaryPath);
  });

  it("runs an explicit fixture CLI for agent discovery without consulting PATH", async () => {
    const repoRoot = await fixtureRepo();
    const report = await inspectDevEnvironment({
      repoRoot,
      env: {
        PATH: process.env.PATH,
        VITE_API_URL: "http://127.0.0.1:18123",
        ...Object.fromEntries(
          INTEGRATION_SECRET_KEYS.map((key, index) => [
            key,
            secretKey(index + 1),
          ]),
        ),
      },
      platform: "darwin",
      arch: "arm64",
      execImpl: execFileAsync,
      fetchImpl: async () => ({
        ok: true,
        status: 200,
        json: async () => ({ status: "ready" }),
      }),
      cacheRoot: join(repoRoot, "cache"),
      toolchainIdentityImpl: () => null,
    });

    expect(report.checks.find(({ id }) => id === "agents")).toMatchObject({
      ok: true,
      message: expect.stringContaining("claude, codex"),
    });
    expect(report.ok).toBe(true);
    expect(report.acceptanceOk).toBe(false);
  });

  it("rejects a malformed discovery summary instead of treating provider detection as ready", async () => {
    const repoRoot = await fixtureRepo();
    const execImpl = vi.fn(async (_binary, args) =>
      args[0] === "version"
        ? { stdout: JSON.stringify({ version: "dev-fixture" }) }
        : {
            stdout: JSON.stringify({
              probe_result: "success",
              runtime_count: 2,
              provider_summary: { codex: 1 },
            }),
          },
    );

    const report = await inspectDevEnvironment({
      repoRoot,
      env: {
        VITE_API_URL: "http://127.0.0.1:18123",
        ...Object.fromEntries(
          INTEGRATION_SECRET_KEYS.map((key, index) => [
            key,
            secretKey(index + 1),
          ]),
        ),
      },
      platform: "darwin",
      arch: "arm64",
      execImpl,
      cacheRoot: join(repoRoot, "cache"),
      toolchainIdentityImpl: () => null,
      fetchImpl: async () => ({
        ok: true,
        status: 200,
        json: async () => ({ status: "ready" }),
      }),
    });

    expect(report.ok).toBe(false);
    expect(report.checks.find(({ id }) => id === "agents")).toMatchObject({
      ok: false,
      message: expect.stringContaining("provider counts do not match"),
    });
    expect(
      report.checks.find(({ id }) => id === "agent-roundtrip"),
    ).toMatchObject({
      status: "pending",
      preflightReady: false,
      localAgentAvailable: false,
      agentExecutionVerified: false,
    });
  });

  it("does not execute a CLI whose checksum no longer matches the manifest", async () => {
    const repoRoot = await fixtureRepo();
    const binaryPath = join(
      repoRoot,
      "apps",
      "desktop",
      "resources",
      "bin",
      "patchbay",
    );
    await writeFile(binaryPath, "tampered after manifest creation");
    const execImpl = vi.fn();

    const report = await inspectDevEnvironment({
      repoRoot,
      env: {
        VITE_API_URL: "http://127.0.0.1:18123",
        ...Object.fromEntries(
          INTEGRATION_SECRET_KEYS.map((key, index) => [
            key,
            secretKey(index + 1),
          ]),
        ),
      },
      platform: "darwin",
      arch: "arm64",
      execImpl,
      cacheRoot: join(repoRoot, "cache"),
      toolchainIdentityImpl: () => null,
      fetchImpl: async () => ({
        ok: true,
        status: 200,
        json: async () => ({ status: "ready" }),
      }),
    });

    expect(report.ok).toBe(false);
    expect(report.checks.find(({ id }) => id === "cli")?.message).toContain(
      "checksum",
    );
    expect(report.checks.find(({ id }) => id === "agents")?.message).toContain(
      "CLI verification failed",
    );
    expect(execImpl).not.toHaveBeenCalled();
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
      cacheRoot: join(repoRoot, "cache"),
      toolchainIdentityImpl: () => null,
      fetchImpl: async () => ({ ok: false, status: 503 }),
    });

    expect(report.ok).toBe(false);
    expect(report.checks.find(({ id }) => id === "backend")?.message).toContain(
      "HTTP 503",
    );
    expect(
      report.checks.find(({ id }) => id === "integrations")?.message,
    ).toContain("PATCHBAY_TELEGRAM_SECRET_KEY");
    expect(
      report.checks.find(({ id }) => id === "integrations")?.providers,
    ).toMatchObject({
      telegram: {
        encryptionKeyConfigured: false,
        providerCredentialStatus: "not_verified",
        messageRoundTripStatus: "not_verified",
      },
      weixin: {
        encryptionKeyConfigured: false,
        providerCredentialStatus: "not_verified",
        messageRoundTripStatus: "not_verified",
      },
    });
  });
});
