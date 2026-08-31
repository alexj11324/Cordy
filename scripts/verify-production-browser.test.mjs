import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  buildGoogleOAuthProbeUrl,
  decodeClerkFrontendApi,
  isExpectedBrowserRequestCancellation,
  requireBrowserReceipt,
  requireGoogleOAuthNavigation,
  requireProtectedNavigation,
} from "./verify-production-browser-contract.mjs";

const SOURCE_SHA = "a".repeat(40);
const EXPECTED_BUILD = `sha-${SOURCE_SHA}`;
const browserVerifierSource = await readFile(
  new URL("./verify-production-browser.mjs", import.meta.url),
  "utf8",
);

test("extracts short-lived browser credentials only from the matching receipt", () => {
  assert.deepEqual(
    requireBrowserReceipt(
      {
        ok: true,
        action: "deploy",
        source_sha: SOURCE_SHA,
        browser_auth: {
          sign_in_ticket: "ticket-value",
          testing_token: "testing-value",
        },
      },
      SOURCE_SHA,
    ),
    { signInTicket: "ticket-value", testingToken: "testing-value" },
  );
  assert.throws(
    () =>
      requireBrowserReceipt(
        { ok: true, action: "deploy", source_sha: "b".repeat(40) },
        SOURCE_SHA,
      ),
    /does not match/u,
  );
});

test("a login redirect is not protected-page acceptance", () => {
  assert.throws(
    () =>
      requireProtectedNavigation({
        url: "https://patchbay.aspectlylabs.com/login",
        status: 307,
        actualBuild: EXPECTED_BUILD,
        expectedBuild: EXPECTED_BUILD,
        expectedPath: "/production-smoke/task-graph",
      }),
    /HTTP 307/u,
  );
});

test("protected acceptance requires the exact route and deployed Web build", () => {
  assert.doesNotThrow(() =>
    requireProtectedNavigation({
      url: "https://patchbay.aspectlylabs.com/production-smoke/issues",
      status: 200,
      actualBuild: EXPECTED_BUILD,
      expectedBuild: EXPECTED_BUILD,
      expectedPath: "/production-smoke/issues",
    }),
  );
  assert.throws(
    () =>
      requireProtectedNavigation({
        url: "https://patchbay.aspectlylabs.com/production-smoke/issues",
        status: 200,
        actualBuild: "sha-old",
        expectedBuild: EXPECTED_BUILD,
        expectedPath: "/production-smoke/issues",
      }),
    /sha-old/u,
  );
});

test("decodes the Clerk Frontend API host without exposing another secret", () => {
  const encoded = Buffer.from("clerk.example.test$", "utf8").toString("base64");
  assert.equal(
    decodeClerkFrontendApi(`pk_live_${encoded}`),
    "clerk.example.test",
  );
  assert.throws(() => decodeClerkFrontendApi("invalid"), /invalid format/u);
});

test("builds a valid desktop OAuth handoff and requires downstream navigation", () => {
  const url = new URL(
    buildGoogleOAuthProbeUrl({
      codeChallenge: "a".repeat(43),
      state: "b".repeat(43),
    }),
  );
  assert.equal(url.origin, "https://accounts.aspectlylabs.com");
  assert.equal(url.pathname, "/oauth/google");
  assert.equal(url.searchParams.get("platform"), "desktop");
  assert.equal(url.searchParams.get("code_challenge"), "a".repeat(43));
  assert.equal(url.searchParams.get("state"), "b".repeat(43));
  assert.throws(
    () => requireGoogleOAuthNavigation(url.href),
    /did not reach accounts\.google\.com/u,
  );
  assert.throws(
    () => requireGoogleOAuthNavigation("https://example.com/oauth"),
    /did not reach accounts\.google\.com/u,
  );
  assert.equal(
    requireGoogleOAuthNavigation("https://accounts.google.com/o/oauth2/auth")
      .hostname,
    "accounts.google.com",
  );
  assert.throws(
    () =>
      buildGoogleOAuthProbeUrl({
        codeChallenge: "too-short",
        state: "b".repeat(43),
      }),
    /valid desktop handoff/u,
  );
});

test("reads the settled page URL after Playwright waitForURL", () => {
  assert.match(
    browserVerifierSource,
    /await downstreamNavigation;\n\s+requireGoogleOAuthNavigation\(page\.url\(\)\);/u,
  );
  assert.doesNotMatch(browserVerifierSource, /downstream\.href/u);
});

test("accepts either a rendered dependency graph or its honest empty state", () => {
  assert.match(
    browserVerifierSource,
    /landmarkLocator\.or\(page\.getByText\(emptyState, \{ exact: true \}\)\)/u,
  );
  assert.match(
    browserVerifierSource,
    /landmark: "Dependency graph canvas",\n\s+emptyState: "No active dependency graphs",/u,
  );
});

test("ignores expected Chromium navigation cancellations only", () => {
  assert.equal(
    isExpectedBrowserRequestCancellation("net::ERR_ABORTED"),
    true,
  );
  assert.equal(isExpectedBrowserRequestCancellation("net::ERR_FAILED"), false);
  assert.equal(isExpectedBrowserRequestCancellation(undefined), false);
  assert.match(
    browserVerifierSource,
    /if \(isExpectedBrowserRequestCancellation\(failure\?\.errorText\)\) return;/u,
  );
});
