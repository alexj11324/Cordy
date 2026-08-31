#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { chmod, copyFile, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  binaryNameForPlatform,
  devRustTargetFor,
  resolveCargoCommand,
} from "./bundle-cli.mjs";
import { loadDevCheckoutEnv } from "./dev-checkout-env.mjs";
import {
  defaultDevCliCacheDir,
  inspectDevRuntimeCache,
  rustBuildEnvironmentFingerprint,
  rustSourceFingerprint,
  rustToolchainIdentity,
} from "./dev-cli-cache.mjs";
import { INTEGRATION_SECRET_KEYS } from "../../../scripts/ensure-dev-integration-secrets.mjs";
import {
  applyDevRuntimeProfile,
  resolveDevRuntimeProfile,
} from "../../../scripts/dev-runtime-profile.mjs";
import {
  bootstrapDevClerkAuth,
  withoutDevClerkEnvironment,
} from "../../../scripts/dev-clerk-auth.mjs";

const execFile = promisify(execFileCallback);
const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..", "..", "..");

async function sha256File(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

export function isValidSecretBoxKey(value) {
  if (typeof value !== "string") return false;
  const normalized = value.trim();
  if (!/^[A-Za-z0-9+/]{43}=$/.test(normalized)) return false;
  try {
    const decoded = Buffer.from(normalized, "base64");
    return decoded.length === 32 && decoded.toString("base64") === normalized;
  } catch {
    return false;
  }
}

export function integrationKeyStatus(env) {
  return Object.fromEntries(
    INTEGRATION_SECRET_KEYS.map((key) => [
      key,
      isValidSecretBoxKey(env[key]),
    ]),
  );
}

export function summarizeDevEnvironmentChecks(checks) {
  const ok = checks.every((check) => check.ok);
  return {
    ok,
    acceptanceOk:
      ok && checks.every((check) => check.status !== "pending"),
  };
}

export function nodeRuntimeCheck(version = process.versions.node) {
  const major = String(version).split(".")[0];
  return major === "22"
    ? {
        id: "node",
        ok: true,
        message: `Node.js ${version} (required major 22)`,
      }
    : {
        id: "node",
        ok: false,
        message: `Node.js ${version} is unsupported (required major 22)`,
        fix: "Activate Node.js 22 from .nvmrc, then rerun `pnpm dev:doctor`.",
      };
}

export function backendUrlFromEnv(env) {
  const configured =
    env.VITE_API_URL ||
    env.NEXT_PUBLIC_API_URL ||
    env.PATCHBAY_PUBLIC_URL ||
    `http://127.0.0.1:${env.PORT || 8080}`;
  return configured.replace(/\/+$/, "");
}

export function accountsUrlFromEnv(env) {
  const configured =
    env.VITE_ACCOUNTS_URL ||
    env.PATCHBAY_DEV_ACCOUNTS_URL ||
    env.FRONTEND_ORIGIN ||
    `http://localhost:${env.FRONTEND_PORT || 3000}`;
  return configured.replace(/\/+$/, "");
}

/**
 * Load the doctor env without letting the checkout file erase a launcher's
 * explicitly selected runtime profile. The doctor runs as a child of both
 * local and hosted launchers, and its env must be the same env Electron gets.
 */
export function loadDoctorEnvironment({
  repoRoot = defaultRepoRoot,
  processEnv = process.env,
  mode,
} = {}) {
  const launcherEnv = { ...processEnv };
  const env = { ...processEnv };
  const launcherMode = mode ?? processEnv.PATCHBAY_DEV_MODE;
  loadDevCheckoutEnv({ repoRoot, env });
  if (launcherMode) {
    Object.assign(env, launcherEnv);
    applyDevRuntimeProfile(
      env,
      resolveDevRuntimeProfile(launcherMode, env),
    );
  }
  return env;
}

function formatBytes(bytes) {
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KiB`;
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

async function probeCliVersion(binaryPath, execImpl) {
  const { stdout } = await execImpl(
    binaryPath,
    ["version", "--output", "json"],
    {
      timeout: 5_000,
    },
  );
  const parsed = JSON.parse(stdout);
  if (typeof parsed.version !== "string" || parsed.version.trim() === "") {
    throw new Error("CLI returned no version");
  }
  return parsed.version;
}

async function probeRuntimeDetection(binaryPath, env, execImpl) {
  const configRoot = await mkdtemp(join(tmpdir(), "patchbay-dev-doctor-"));
  try {
    const { stdout } = await execImpl(
      binaryPath,
      ["daemon", "probe-runtimes", "--profile", "desktop-dev-doctor"],
      {
        timeout: 15_000,
        env: {
          ...env,
          PATCHBAY_TASK_CONFIG_ROOT: configRoot,
        },
      },
    );
    const parsed = JSON.parse(stdout);
    if (parsed.probe_result !== "success") {
      throw new Error(
        `runtime probe returned ${parsed.probe_result || "unknown"}`,
      );
    }
    return parsed;
  } finally {
    await rm(configRoot, { recursive: true, force: true });
  }
}

async function probeBackend(apiUrl, fetchImpl) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 3_000);
  try {
    const response = await fetchImpl(`${apiUrl}/healthz`, {
      signal: controller.signal,
    });
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    const payload = await response.json();
    if (payload.status !== "ready") {
      throw new Error(`unexpected status ${payload.status || "unknown"}`);
    }
    return payload;
  } finally {
    clearTimeout(timeout);
  }
}

async function probeHostedAccounts(accountsUrl, fetchImpl) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 3_000);
  try {
    const response = await fetchImpl(`${accountsUrl}/readyz`, {
      signal: controller.signal,
    });
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    const payload = await response.json();
    if (payload.status !== "ready" && payload.status !== "ok") {
      throw new Error(`unexpected status ${payload.status || "unknown"}`);
    }
    return payload;
  } finally {
    clearTimeout(timeout);
  }
}

export async function inspectDevEnvironment({
  repoRoot = defaultRepoRoot,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
  fetchImpl = fetch,
  execImpl = execFile,
  cacheRoot = defaultDevCliCacheDir({ env, platform }),
} = {}) {
  const binaryName = binaryNameForPlatform(platform);
  const binaryPath = join(
    repoRoot,
    "apps",
    "desktop",
    "resources",
    "bin",
    binaryName,
  );
  const manifestPath = `${binaryPath}.dev-manifest.json`;
  const sourceFingerprint = rustSourceFingerprint(repoRoot);
  const rustTarget = devRustTargetFor(platform, arch);
  const apiUrl = backendUrlFromEnv(env);
  const hosted = env.PATCHBAY_DEV_MODE === "hosted";
  const accountsUrl = accountsUrlFromEnv(env);
  const checks = [];

  const cache = await inspectDevRuntimeCache({ cacheRoot });
  const currentCached = cache.completeFingerprints.some(
    (entry) =>
      entry.rustTarget === rustTarget &&
      entry.sourceFingerprint === sourceFingerprint,
  );
  checks.push({
    id: "cache",
    ok: true,
    message: currentCached
      ? `dev runtime cache has current source; ${cache.completeFingerprintCount} complete fingerprints, ${formatBytes(cache.totalBytes)}`
      : `dev runtime cache does not contain the current source; ${cache.completeFingerprintCount} complete fingerprints, ${formatBytes(cache.totalBytes)}`,
    ...(!currentCached && {
      fix: "Run `pnpm dev`; a cache miss performs one worktree-local incremental Rust build. Use `pnpm dev:cache:prune` to remove stale entries.",
    }),
  });

  let manifest;
  let cliProbeRoot;
  let verifiedBinaryPath;
  let cliVerified = false;
  let cliIdentityPending = false;
  try {
    manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    if (
      manifest.sourceFingerprint !== sourceFingerprint ||
      manifest.rustTarget !== rustTarget ||
      manifest.profile !== "dev"
    ) {
      throw new Error(
        "manifest does not match current Rust source/target/profile",
      );
    }
    const expectedBuildEnvironment =
      manifest.buildVariables?.environmentFingerprint;
    if (
      expectedBuildEnvironment &&
      expectedBuildEnvironment !==
        rustBuildEnvironmentFingerprint(env, rustTarget, "dev")
    ) {
      throw new Error("binary build environment does not match this checkout");
    }
    const manifestToolchain = manifest.toolchainIdentity;
    if (!manifestToolchain || manifestToolchain === "unavailable") {
      cliIdentityPending = true;
    } else {
      const cargoCommand = resolveCargoCommand(env, platform);
      const currentToolchain = rustToolchainIdentity(env, cargoCommand, {
        platform,
        cwd: join(repoRoot, "server-rs"),
      });
      if (!currentToolchain) {
        cliIdentityPending = true;
      } else if (currentToolchain !== manifestToolchain) {
        throw new Error("binary Rust toolchain does not match this checkout");
      }
    }
    cliProbeRoot = await mkdtemp(join(tmpdir(), "patchbay-dev-cli-probe-"));
    const copiedBinaryPath = join(cliProbeRoot, binaryName);
    await copyFile(binaryPath, copiedBinaryPath);
    if (platform !== "win32") await chmod(copiedBinaryPath, 0o755);
    if ((await sha256File(copiedBinaryPath)) !== manifest.sha256) {
      throw new Error("binary checksum does not match its source manifest");
    }
    const version = await probeCliVersion(copiedBinaryPath, execImpl);
    if (version !== manifest.buildVariables?.version) {
      throw new Error("binary version does not match its source manifest");
    }
    verifiedBinaryPath = copiedBinaryPath;
    cliVerified = true;
    checks.push({
      id: "cli",
      ok: true,
      ...(cliIdentityPending && { status: "pending" }),
      message: cliIdentityPending
        ? `source-matched dev CLI ${version} (${sourceFingerprint.slice(0, 12)}); Rust toolchain identity is not available, so cache compatibility is pending`
        : `source-matched dev CLI ${version} (${sourceFingerprint.slice(0, 12)})`,
      ...(cliIdentityPending && {
        fix: "Install/activate the Rust toolchain used by this checkout, or rebuild the source-matched CLI with `pnpm dev`.",
      }),
    });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    checks.push({
      id: "cli",
      ok: false,
      message: `source-matched dev CLI unavailable: ${detail}`,
      fix: "Run `pnpm dev`; a cache miss will perform one incremental Rust dev build.",
    });
  }

  try {
    await probeBackend(apiUrl, fetchImpl);
    checks.push({
      id: "backend",
      ok: true,
      message: hosted
        ? `hosted API ready at ${apiUrl}`
        : `backend and database ready at ${apiUrl}`,
    });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    checks.push({
      id: "backend",
      ok: false,
      message: hosted
        ? `hosted API not ready at ${apiUrl}: ${detail}`
        : `backend/database not ready at ${apiUrl}: ${detail}`,
      fix: hosted
        ? "Check https://api.aspectlylabs.com/healthz and retry `pnpm dev:hosted`."
        : "Use the complete `pnpm dev` entry; inspect the preceding migration/backend logs.",
    });
  }

  if (hosted) {
    try {
      await probeHostedAccounts(accountsUrl, fetchImpl);
      checks.push({
        id: "accounts",
        ok: true,
        message: `hosted accounts broker ready at ${accountsUrl}`,
      });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      checks.push({
        id: "accounts",
        ok: false,
        message: `hosted accounts broker not ready at ${accountsUrl}: ${detail}`,
        fix: "Check https://accounts.aspectlylabs.com/readyz and retry `pnpm dev:hosted`.",
      });
    }
  }

  if (!cliVerified || !verifiedBinaryPath) {
    checks.push({
      id: "agents",
      ok: false,
      message:
        "agent detection unavailable because source-matched CLI verification failed",
      fix: "Rebuild the source-matched CLI with `pnpm dev`, then rerun `pnpm dev:doctor`.",
    });
  } else {
    try {
      const report = await probeRuntimeDetection(
        verifiedBinaryPath,
        env,
        execImpl,
      );
      const providers = Object.keys(report.provider_summary || {});
      checks.push({
        id: "agents",
        ok: true,
        message:
          providers.length > 0
            ? `agent detection available: ${providers.join(", ")}`
            : "agent detection available; no supported local agent CLI was found",
      });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      checks.push({
        id: "agents",
        ok: false,
        message: `agent detection probe failed: ${detail}`,
        fix: "Rebuild the source-matched CLI with `pnpm dev`, then rerun `pnpm dev:doctor`.",
      });
    }
  }

  if (cliProbeRoot) {
    await rm(cliProbeRoot, { recursive: true, force: true });
  }

  const keyStatus = integrationKeyStatus(env);
  const missingKeys = Object.entries(keyStatus)
    .filter(([, configured]) => !configured)
    .map(([key]) => key);
  checks.push(
    missingKeys.length === 0
      ? {
          id: "integrations",
          ok: true,
          status: "pending",
          providerAccountsVerified: false,
          messageRoundTripsVerified: false,
          message:
            "credential encryption is ready; provider account credentials and message round-trips are not verified by this doctor",
          fix: "After Electron login, open Settings → Integrations and complete the Telegram/WeChat connect flow; use the provider status there to verify a real account.",
        }
      : {
          id: "integrations",
          ok: false,
          message: `integration encryption configuration missing/invalid: ${missingKeys.join(", ")}`,
          fix: "Run `pnpm dev`; it generates local-only keys in the checkout env file without logging them.",
        },
  );

  return {
    apiUrl,
    binaryPath,
    checks,
    ...summarizeDevEnvironmentChecks(checks),
  };
}

export function printDevEnvironmentReport(report, log = console) {
  for (const check of report.checks) {
    const pending = check.ok && check.status === "pending";
    const method = check.ok ? (pending ? "warn" : "log") : "error";
    const symbol = check.ok ? (pending ? "!" : "✓") : "✗";
    log[method](`${symbol} ${check.message}`);
    if (check.fix) log[method](`  Fix: ${check.fix}`);
  }
  if (report.ok && report.acceptanceOk === false) {
    log.warn(
      "! Development runtime prerequisites are ready, but one or more acceptance checks remain pending.",
    );
  }
}

async function main() {
  const warnOnly = process.argv.includes("--warn-only");
  const env = loadDoctorEnvironment({
    mode: process.argv.includes("--hosted") ? "hosted" : undefined,
  });
  const preflightChecks = [nodeRuntimeCheck()];
  if (env.PATCHBAY_DEV_MODE !== "hosted") {
    try {
      const auth = await bootstrapDevClerkAuth({ env });
      preflightChecks.push({
        id: "clerk",
        ok: true,
        message: `Clerk development authentication ready for ${auth.authorizedParties} (${auth.source})`,
      });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      preflightChecks.push({
        id: "clerk",
        ok: false,
        message: `Clerk development authentication unavailable: ${detail}`,
        fix: "Authenticate gcloud for Secret Manager access, or provide the complete Clerk development variables in the process environment; never write them to .env files.",
      });
    }
  } else {
    preflightChecks.push({
      id: "clerk",
      ok: true,
      status: "pending",
      message: "Hosted mode delegates authentication to the hosted account origin",
    });
  }
  const report = await inspectDevEnvironment({
    env: withoutDevClerkEnvironment(env),
  });
  report.checks.unshift(...preflightChecks);
  Object.assign(report, summarizeDevEnvironmentChecks(report.checks));
  printDevEnvironmentReport(report);
  if (!report.ok && !warnOnly) process.exitCode = 1;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    const detail = error instanceof Error ? error.message : String(error);
    console.error(`✗ Desktop development doctor failed: ${detail}`);
    process.exitCode = 1;
  });
}
