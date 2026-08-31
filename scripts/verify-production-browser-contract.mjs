const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/u;
const GOOGLE_OAUTH_ENTRY = "https://accounts.aspectlylabs.com/oauth/google";

export function requiredString(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${label} is required`);
  }
  return value.trim();
}

export function decodeClerkFrontendApi(publishableKey) {
  const key = requiredString(publishableKey, "CLERK_PUBLISHABLE_KEY");
  const match = /^pk_(?:live|test)_(.+)$/u.exec(key);
  if (!match) throw new Error("CLERK_PUBLISHABLE_KEY has an invalid format");
  const decoded = Buffer.from(match[1], "base64")
    .toString("utf8")
    .replace(/\$$/u, "");
  if (!/^[a-z0-9.-]+$/iu.test(decoded) || !decoded.includes(".")) {
    throw new Error(
      "CLERK_PUBLISHABLE_KEY does not contain a valid Frontend API host",
    );
  }
  return decoded;
}

export function requireBrowserReceipt(receipt, sourceSha) {
  if (!SHA_PATTERN.test(sourceSha)) {
    throw new Error("source SHA must be 40 lowercase hexadecimal characters");
  }
  if (
    receipt?.ok !== true ||
    receipt?.action !== "deploy" ||
    receipt?.source_sha !== sourceSha
  ) {
    throw new Error(
      "deployment receipt does not match the requested source SHA",
    );
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

export function requireProtectedNavigation({
  url,
  status,
  actualBuild,
  expectedBuild,
  expectedPath,
}) {
  const parsed = new URL(url);
  if (status !== 200) {
    throw new Error(`${url} returned HTTP ${status}, expected 200`);
  }
  if (parsed.pathname !== expectedPath) {
    throw new Error(
      `${url} ended at ${parsed.pathname}, expected ${expectedPath}`,
    );
  }
  if (actualBuild !== expectedBuild) {
    throw new Error(
      `${url} reported build ${actualBuild ?? "<missing>"}, expected ${expectedBuild}`,
    );
  }
}

export function isActionableBrowserRequestFailure(errorText) {
  // Chromium reports requests canceled by a page navigation as ERR_ABORTED.
  // Those requests did not fail at the network boundary; HTTP errors and all
  // other transport failures remain actionable in the verifier.
  return errorText !== "net::ERR_ABORTED";
}

export function buildGoogleOAuthProbeUrl({ codeChallenge, state }) {
  if (
    !HANDOFF_VALUE_PATTERN.test(codeChallenge) ||
    !HANDOFF_VALUE_PATTERN.test(state)
  ) {
    throw new Error("Google OAuth probe requires a valid desktop handoff");
  }
  const url = new URL(GOOGLE_OAUTH_ENTRY);
  url.search = new URLSearchParams({
    platform: "desktop",
    code_challenge: codeChallenge,
    state,
  }).toString();
  return url.href;
}

export function requireGoogleOAuthNavigation(url) {
  const parsed = new URL(url);
  if (
    parsed.protocol !== "https:" ||
    parsed.hostname !== "accounts.google.com"
  ) {
    throw new Error(
      `Google OAuth did not reach accounts.google.com (ended at ${parsed.origin})`,
    );
  }
  return parsed;
}
