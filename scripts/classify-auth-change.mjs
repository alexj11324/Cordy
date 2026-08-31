import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const AUTH_BROKER_RELEASE_PATTERNS = [
  /^apps\/auth-broker\//,
  /^contracts\/auth-broker\//,
  /^deploy\/helm\/patchbay-auth-broker\//,
  /^\.dockerignore$/,
  /^Dockerfile\.auth-broker$/,
  /^\.github\/workflows\/auth-broker-release\.yml$/,
];

const FULL_GOOGLE_OAUTH_E2E_PATTERNS = [
  /^contracts\/auth-broker\//,
  /^deploy\/cloudflare\/accounts-origin-proxy\//,
  /^apps\/auth-broker\/app\/oauth\/google\//,
  /^apps\/auth-broker\/app\/login\//,
  /^apps\/auth-broker\/app\/v1\/desktop\//,
  /^apps\/auth-broker\/lib\/(broker-client|contract|desktop-handoff|google-oauth|runtime-config|rust-api-proxy)\.(ts|tsx)$/,
  /^deploy\/helm\/patchbay-auth-broker\/values\.yaml$/,
  /^deploy\/helm\/patchbay-auth-broker\/templates\/deployment\.yaml$/,
  /^apps\/web\/app\/oauth\/google\//,
  /^apps\/web\/app\/\(auth\)\/login\//,
  /^apps\/web\/features\/auth\//,
  /^server-rs\/crates\/patchbay-handler\/src\/(clerk_auth|desktop_google|desktop_handoff)\.rs$/,
  /^server-rs\/crates\/patchbay-handler\/src\/routes\/auth\.rs$/,
  /^packages\/core\/api\/client\.ts$/,
  /^apps\/desktop\/src\/renderer\/src\/pages\/(login-handoff|login-url)\.ts$/,
  /^apps\/desktop\/src\/main\/runtime-config-loader\.ts$/,
  /^apps\/desktop\/src\/shared\/runtime-config\.ts$/,
];

export function classifyAuthChange(paths) {
  const normalized = [...new Set(paths.map((path) => path.trim()).filter(Boolean))];
  return {
    authBrokerRelease: normalized.some((path) =>
      AUTH_BROKER_RELEASE_PATTERNS.some((pattern) => pattern.test(path)),
    ),
    fullGoogleOAuthE2E: normalized.some((path) =>
      FULL_GOOGLE_OAUTH_E2E_PATTERNS.some((pattern) => pattern.test(path)),
    ),
  };
}

function changedPaths(base, head) {
  return execFileSync("git", ["diff", "--name-only", `${base}...${head}`], {
    encoding: "utf8",
  })
    .split("\n")
    .filter(Boolean);
}

function run() {
  const base = process.argv[2];
  const head = process.argv[3] ?? "HEAD";
  if (!base) {
    throw new Error("usage: node scripts/classify-auth-change.mjs <base> [head]");
  }
  const result = classifyAuthChange(changedPaths(base, head));
  const output = [
    `auth_broker_release=${String(result.authBrokerRelease)}`,
    `full_google_oauth_e2e=${String(result.fullGoogleOAuthE2E)}`,
  ].join("\n");
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${output}\n`);
  process.stdout.write(`${output}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) run();
