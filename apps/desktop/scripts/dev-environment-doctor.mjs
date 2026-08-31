#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";

import { binaryNameForPlatform, rustTargetFor } from "./bundle-cli.mjs";
import { rustSourceFingerprint } from "./dev-cli-cache.mjs";

const execFile = promisify(execFileCallback);
const here = dirname(fileURLToPath(import.meta.url));
const defaultRepoRoot = resolve(here, "..", "..", "..");
const REQUIRED_INTEGRATION_KEYS = [
  "PATCHBAY_TELEGRAM_SECRET_KEY",
  "PATCHBAY_WEIXIN_SECRET_KEY",
];

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
    REQUIRED_INTEGRATION_KEYS.map((key) => [
      key,
      isValidSecretBoxKey(env[key]),
    ]),
  );
}

export function backendUrlFromEnv(env) {
  const configured =
    env.VITE_API_URL ||
    env.NEXT_PUBLIC_API_URL ||
    env.PATCHBAY_PUBLIC_URL ||
    `http://127.0.0.1:${env.PORT || 8080}`;
  return configured.replace(/\/+$/, "");
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

export async function inspectDevEnvironment({
  repoRoot = defaultRepoRoot,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
  fetchImpl = fetch,
  execImpl = execFile,
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
  const rustTarget = rustTargetFor(platform, arch);
  const apiUrl = backendUrlFromEnv(env);
  const checks = [];

  let manifest;
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
    const version = await probeCliVersion(binaryPath, execImpl);
    if (
      (await sha256File(binaryPath)) !== manifest.sha256 ||
      version !== manifest.buildVariables?.version
    ) {
      throw new Error(
        "binary checksum/version does not match its source manifest",
      );
    }
    checks.push({
      id: "cli",
      ok: true,
      message: `source-matched dev CLI ${version} (${sourceFingerprint.slice(0, 12)})`,
    });
  } catch (error) {
    checks.push({
      id: "cli",
      ok: false,
      message: `source-matched dev CLI unavailable: ${error.message}`,
      fix: "Run `pnpm dev`; a cache miss will perform one incremental Rust dev build.",
    });
  }

  try {
    await probeBackend(apiUrl, fetchImpl);
    checks.push({
      id: "backend",
      ok: true,
      message: `backend and database ready at ${apiUrl}`,
    });
  } catch (error) {
    checks.push({
      id: "backend",
      ok: false,
      message: `backend/database not ready at ${apiUrl}: ${error.message}`,
      fix: "Use the complete `pnpm dev` entry; inspect the preceding migration/backend logs.",
    });
  }

  try {
    const report = await probeRuntimeDetection(binaryPath, env, execImpl);
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
    checks.push({
      id: "agents",
      ok: false,
      message: `agent detection probe failed: ${error.message}`,
      fix: "Rebuild the source-matched CLI with `pnpm dev`, then rerun `pnpm dev:doctor`.",
    });
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
          message:
            "Telegram and Weixin credential encryption are enabled; account credentials remain UI-supplied",
        }
      : {
          id: "integrations",
          ok: false,
          message: `integration encryption configuration missing/invalid: ${missingKeys.join(", ")}`,
          fix: "Run `pnpm dev`; it generates local-only keys in the checkout env file without logging them.",
        },
  );

  return { apiUrl, binaryPath, checks, ok: checks.every((check) => check.ok) };
}

export function printDevEnvironmentReport(report, log = console) {
  for (const check of report.checks) {
    const method = check.ok ? "log" : "error";
    log[method](`${check.ok ? "✓" : "✗"} ${check.message}`);
    if (check.fix) log[method](`  Fix: ${check.fix}`);
  }
}

async function main() {
  const warnOnly = process.argv.includes("--warn-only");
  const report = await inspectDevEnvironment();
  printDevEnvironmentReport(report);
  if (!report.ok && !warnOnly) process.exitCode = 1;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    console.error(`✗ Desktop development doctor failed: ${error.message}`);
    process.exitCode = 1;
  });
}
