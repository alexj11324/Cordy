#!/usr/bin/env node

import { randomBytes } from "node:crypto";
import { chmod, readFile, rename, writeFile } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

export const INTEGRATION_SECRET_KEYS = [
  "PATCHBAY_LARK_SECRET_KEY",
  "PATCHBAY_SLACK_SECRET_KEY",
  "PATCHBAY_DINGTALK_SECRET_KEY",
  "PATCHBAY_WECOM_SECRET_KEY",
  "PATCHBAY_TELEGRAM_SECRET_KEY",
  "PATCHBAY_WEIXIN_SECRET_KEY",
];

function isValidKey(value) {
  const normalized = value?.trim();
  if (!normalized || !/^[A-Za-z0-9+/]{43}=$/.test(normalized)) return false;
  try {
    const decoded = Buffer.from(normalized, "base64");
    return decoded.length === 32 && decoded.toString("base64") === normalized;
  } catch {
    return false;
  }
}

export function ensureLocalIntegrationSecrets(
  contents,
  generate = () => randomBytes(32).toString("base64"),
) {
  let next = contents.endsWith("\n") ? contents : `${contents}\n`;
  const generated = [];

  for (const key of INTEGRATION_SECRET_KEYS) {
    const pattern = new RegExp(`^${key}=(.*)$`, "m");
    const match = next.match(pattern);
    if (isValidKey(match?.[1])) continue;
    if (match?.[1]?.trim()) {
      throw new Error(
        `${key} already has an invalid non-empty value; fix or clear it explicitly`,
      );
    }
    const value = generate();
    next = match
      ? next.replace(pattern, `${key}=${value}`)
      : `${next}${key}=${value}\n`;
    generated.push(key);
  }
  return { contents: next, generated };
}

export async function ensureSecretsFile(envFile) {
  const current = await readFile(envFile, "utf8");
  const result = ensureLocalIntegrationSecrets(current);
  if (result.generated.length === 0) return result;

  const temporary = join(
    dirname(envFile),
    `.${basename(envFile)}.tmp-${process.pid}`,
  );
  await writeFile(temporary, result.contents, { mode: 0o600 });
  await rename(temporary, envFile);
  await chmod(envFile, 0o600);
  return result;
}

async function main() {
  const envFile = process.argv[2];
  if (!envFile)
    throw new Error("usage: ensure-dev-integration-secrets.mjs <env-file>");
  const result = await ensureSecretsFile(envFile);
  if (result.generated.length > 0) {
    console.log(
      `[dev] generated local-only integration encryption keys in ${envFile}: ${result.generated.join(", ")}`,
    );
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    console.error(
      `[dev] failed to prepare integration secrets: ${error.message}`,
    );
    process.exitCode = 1;
  });
}
