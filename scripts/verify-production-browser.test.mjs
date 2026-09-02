import assert from "node:assert/strict";
import test from "node:test";

import {
  ACCOUNTS_ORIGIN,
  API_ORIGIN,
  buildGoogleOAuthProbeUrl,
  buildPkceChallenge,
  PRODUCT_ORIGIN,
  requireBrowserReceipt,
  requireBuildHeaders,
  requireClerkPublishableKey,
  requireDesktopCompletion,
  requireGoogleOAuthNavigation,
  requireRedeemedSession,
} from "./verify-production-browser-contract.mjs";

const SOURCE_SHA = "a".repeat(40);

test("uses only the three public Aspectly Labs product origins", () => {
  assert.equal(API_ORIGIN, "https://api.aspectlylabs.com");
  assert.equal(PRODUCT_ORIGIN, "https://patchbay.aspectlylabs.com");
  assert.equal(ACCOUNTS_ORIGIN, "https://accounts.aspectlylabs.com");
});

test("builds a PKCE-bound direct Accounts Google OAuth entry", () => {
  const verifier = "v".repeat(43);
  const challenge = buildPkceChallenge(verifier);
  const url = new URL(
    buildGoogleOAuthProbeUrl({
      codeChallenge: challenge,
      state: "s".repeat(43),
    }),
  );
  assert.equal(url.origin, ACCOUNTS_ORIGIN);
  assert.equal(url.pathname, "/oauth/google");
  assert.equal(url.searchParams.get("platform"), "desktop");
  assert.equal(url.searchParams.get("code_challenge"), challenge);
  assert.equal(url.searchParams.get("state"), "s".repeat(43));
  assert.equal(url.searchParams.has("app_origin"), false);
  assert.equal(url.href.includes("localhost"), false);
});

test("requires Google and not a lookalike OAuth destination", () => {
  assert.equal(
    requireGoogleOAuthNavigation("https://accounts.google.com/o/oauth2/auth")
      .hostname,
    "accounts.google.com",
  );
  assert.throws(
    () => requireGoogleOAuthNavigation("https://accounts.google.example/auth"),
    /did not reach accounts\.google\.com/u,
  );
});

test("requires matching build and commit headers", () => {
  const headers = new Headers({
    "x-patchbay-build": `sha-${SOURCE_SHA}`,
    "x-patchbay-commit": SOURCE_SHA,
  });
  assert.doesNotThrow(() =>
    requireBuildHeaders(headers, SOURCE_SHA, "runtime"),
  );
  headers.set("x-patchbay-commit", "b".repeat(40));
  assert.throws(
    () => requireBuildHeaders(headers, SOURCE_SHA, "runtime"),
    /reported commit/u,
  );
});

test("accepts credentials only from the matching deployment receipt", () => {
  const receipt = {
    ok: true,
    action: "deploy",
    source_sha: SOURCE_SHA,
    browser_auth: {
      sign_in_ticket: "ticket",
      testing_token: "testing",
    },
  };
  assert.deepEqual(requireBrowserReceipt(receipt, SOURCE_SHA), {
    signInTicket: "ticket",
    testingToken: "testing",
  });
  assert.throws(
    () => requireBrowserReceipt(receipt, "b".repeat(40)),
    /does not match/u,
  );
});

test("validates one-time broker completion and redemption payloads", () => {
  assert.equal(
    requireDesktopCompletion({
      callback_protocol: "patchbay",
      code: `pbd_${"c".repeat(43)}`,
    }),
    `pbd_${"c".repeat(43)}`,
  );
  assert.throws(
    () =>
      requireDesktopCompletion({
        callback_protocol: "http",
        code: `pbd_${"c".repeat(43)}`,
      }),
    /invalid desktop completion/u,
  );
  assert.equal(requireRedeemedSession({ token: "jwt-value" }), "jwt-value");
  assert.throws(
    () => requireRedeemedSession({ token: "bad\nvalue" }),
    /invalid/u,
  );
});

test("refuses to open a browser without a real Clerk publishable key", () => {
  assert.equal(
    requireClerkPublishableKey("pk_live_Y2xlcmsuZXhhbXBsZS5jb20k"),
    "pk_live_Y2xlcmsuZXhhbXBsZS5jb20k",
  );
  for (const value of [undefined, "", "   ", "sk_live_secret", "pk_live_"]) {
    assert.throws(
      () => requireClerkPublishableKey(value),
      /CLERK_PUBLISHABLE_KEY/u,
      `expected ${JSON.stringify(value)} to be rejected`,
    );
  }
});
