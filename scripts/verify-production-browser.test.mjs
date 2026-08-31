import assert from "node:assert/strict";
import test from "node:test";

import {
  decodeClerkFrontendApi,
  requireBrowserReceipt,
  requireProtectedNavigation,
} from "./verify-production-browser-contract.mjs";

const SOURCE_SHA = "a".repeat(40);
const EXPECTED_BUILD = `sha-${SOURCE_SHA}`;

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
