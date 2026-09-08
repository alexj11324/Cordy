import { createHash } from "node:crypto";

const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/u;
const DESKTOP_CODE_PATTERN = /^ovd_[A-Za-z0-9_-]{43}$/u;

const ORIGINS = {
  production: {
    product: "https://orvilo.aspectlylabs.com",
    api: "https://api.aspectlylabs.com",
    accounts: "https://accounts.aspectlylabs.com",
    desktopCallbackProtocol: "orvilo",
  },
  staging: {
    product: "https://staging.aspectlylabs.com",
    api: "https://api.staging.aspectlylabs.com",
    accounts: "https://accounts.staging.aspectlylabs.com",
    // Fixture scheme for the headless Desktop handoff. Unpackaged Staging
    // uses orvilo-staging-<16 hex>://; this value is allow-listed and must
    // never fall back to the production orvilo:// handler.
    desktopCallbackProtocol: "orvilo-staging-aaaaaaaaaaaaaaaa",
  },
};

export function hostedBrowserOrigins(
  env = process.env.ORVILO_VERIFY_ENV ?? "production",
) {
  const origins = ORIGINS[env];
  if (!origins) {
    throw new Error(`unsupported browser verification environment: ${env}`);
  }
  return {
    env,
    product: origins.product,
    api: origins.api,
    accounts: origins.accounts,
    cookieDomain: new URL(origins.product).hostname,
    desktopCallbackProtocol: origins.desktopCallbackProtocol,
  };
}

const hosted = hostedBrowserOrigins();
export const DEPLOYMENT_ENV = hosted.env;
export const PRODUCT_ORIGIN = hosted.product;
export const API_ORIGIN = hosted.api;
export const ACCOUNTS_ORIGIN = hosted.accounts;
export const PRODUCT_COOKIE_DOMAIN = hosted.cookieDomain;
export const DESKTOP_CALLBACK_PROTOCOL = hosted.desktopCallbackProtocol;

export function requiredString(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${label} is required`);
  }
  return value.trim();
}

export function requireSourceSha(value) {
  if (!SHA_PATTERN.test(value)) {
    throw new Error("source SHA must be 40 lowercase hexadecimal characters");
  }
  return value;
}

export function buildPkceChallenge(verifier) {
  const value = requiredString(verifier, "PKCE verifier");
  if (!HANDOFF_VALUE_PATTERN.test(value)) {
    throw new Error("PKCE verifier has an invalid format");
  }
  return createHash("sha256").update(value).digest("base64url");
}

export function buildGoogleOAuthProbeUrl({ codeChallenge, state }) {
  if (
    !HANDOFF_VALUE_PATTERN.test(codeChallenge) ||
    !HANDOFF_VALUE_PATTERN.test(state)
  ) {
    throw new Error("Google OAuth probe requires a valid desktop handoff");
  }
  const url = new URL("/oauth/google", ACCOUNTS_ORIGIN);
  url.search = new URLSearchParams({
    platform: "desktop",
    code_challenge: codeChallenge,
    state,
  }).toString();
  return url.href;
}

export function buildAccountsLoginProbeUrl({ codeChallenge, state }) {
  if (
    !HANDOFF_VALUE_PATTERN.test(codeChallenge) ||
    !HANDOFF_VALUE_PATTERN.test(state)
  ) {
    throw new Error("Accounts login probe requires a valid desktop handoff");
  }
  const url = new URL("/login", ACCOUNTS_ORIGIN);
  url.search = new URLSearchParams({
    platform: "desktop",
    state,
    code_challenge: codeChallenge,
  }).toString();
  return url.href;
}

export function requireGoogleOAuthNavigation(rawUrl) {
  const url = new URL(rawUrl);
  if (url.protocol !== "https:" || url.hostname !== "accounts.google.com") {
    throw new Error(
      `Google OAuth did not reach accounts.google.com (ended at ${url.origin})`,
    );
  }
  return url;
}

export function requireBuildHeaders(headers, sourceSha, label) {
  const sha = requireSourceSha(sourceSha);
  const expectedBuild = `sha-${sha}`;
  const get = (name) =>
    typeof headers?.get === "function" ? headers.get(name) : headers?.[name];
  const build = get("x-orvilo-build");
  const commit = get("x-orvilo-commit");
  if (build !== expectedBuild) {
    throw new Error(
      `${label} reported build ${build ?? "<missing>"}, expected ${expectedBuild}`,
    );
  }
  if (commit !== sha) {
    throw new Error(
      `${label} reported commit ${commit ?? "<missing>"}, expected ${sha}`,
    );
  }
}

// @clerk/testing derives the Clerk Frontend API host from the publishable key
// and refuses to install its testing-token route without one. The deploy job
// supplies the key; validate its shape here so a missing or malformed secret
// fails before a browser is launched instead of surfacing as an opaque
// "setup testing token" error mid-run.
export function requireClerkPublishableKey(value) {
  const key = requiredString(value, "CLERK_PUBLISHABLE_KEY");
  if (!/^pk_(live|test)_[A-Za-z0-9+/=_-]+$/u.test(key)) {
    throw new Error("CLERK_PUBLISHABLE_KEY is not a Clerk publishable key");
  }
  return key;
}

export function requireBrowserReceipt(receipt, sourceSha) {
  const sha = requireSourceSha(sourceSha);
  if (
    receipt?.ok !== true ||
    receipt?.action !== "deploy" ||
    receipt?.source_sha !== sha
  ) {
    throw new Error("deployment receipt does not match the requested source SHA");
  }
  return {
    signInTicket: requiredString(
      receipt.browser_auth?.sign_in_ticket,
      "browser sign-in ticket",
    ),
    testingToken: requiredString(
      receipt.browser_auth?.testing_token,
      "browser testing token",
    ),
  };
}

export function requireDesktopCompletion(
  payload,
  expectedProtocol = DESKTOP_CALLBACK_PROTOCOL,
) {
  if (
    !payload ||
    typeof payload !== "object" ||
    !DESKTOP_CODE_PATTERN.test(payload.code) ||
    payload.callback_protocol !== expectedProtocol
  ) {
    throw new Error("Accounts broker returned an invalid desktop completion");
  }
  return payload.code;
}

export function requireRedeemedSession(payload) {
  const token = requiredString(payload?.token, "redeemed Orvilo session");
  if (token.length > 8192 || /[\r\n]/u.test(token)) {
    throw new Error("redeemed Orvilo session is invalid");
  }
  return token;
}
