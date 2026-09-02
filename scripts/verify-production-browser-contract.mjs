import { createHash } from "node:crypto";

const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/u;
const DESKTOP_CODE_PATTERN = /^pbd_[A-Za-z0-9_-]{43}$/u;

export const PRODUCT_ORIGIN = "https://patchbay.aspectlylabs.com";
export const API_ORIGIN = "https://api.aspectlylabs.com";
export const ACCOUNTS_ORIGIN = "https://accounts.aspectlylabs.com";

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
  const build = get("x-patchbay-build");
  const commit = get("x-patchbay-commit");
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

export function requireDesktopCompletion(payload) {
  if (
    !payload ||
    typeof payload !== "object" ||
    !DESKTOP_CODE_PATTERN.test(payload.code) ||
    payload.callback_protocol !== "patchbay"
  ) {
    throw new Error("Accounts broker returned an invalid desktop completion");
  }
  return payload.code;
}

export function requireRedeemedSession(payload) {
  const token = requiredString(payload?.token, "redeemed Patchbay session");
  if (token.length > 8192 || /[\r\n]/u.test(token)) {
    throw new Error("redeemed Patchbay session is invalid");
  }
  return token;
}
