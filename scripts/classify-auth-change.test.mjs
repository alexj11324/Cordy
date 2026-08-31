import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { classifyAuthChange } from "./classify-auth-change.mjs";

const ciWorkflow = readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);

test("ordinary product updates do not require an auth broker release or full OAuth E2E", () => {
  for (const path of [
    "apps/web/app/(dashboard)/page.tsx",
    "server-rs/crates/patchbay-handler/src/issues.rs",
    "apps/desktop/src/renderer/components/sidebar.tsx",
    "packages/views/src/pages/inbox.tsx",
  ]) {
    assert.deepEqual(classifyAuthChange([path]), {
      authBrokerRelease: false,
      fullGoogleOAuthE2E: false,
    });
  }
});

test("broker packaging changes remain in the independent release lane", () => {
  for (const path of [
    "Dockerfile.auth-broker",
    ".dockerignore",
    "deploy/helm/patchbay-auth-broker/Chart.yaml",
    ".github/workflows/auth-broker-release.yml",
    "apps/auth-broker/app/globals.css",
  ]) {
    assert.deepEqual(classifyAuthChange([path]), {
      authBrokerRelease: true,
      fullGoogleOAuthE2E: false,
    });
  }
});

test("protocol, provider callback, session exchange, and desktop boundaries require full OAuth E2E", () => {
  for (const path of [
    "contracts/auth-broker/v1.json",
    "deploy/cloudflare/accounts-origin-proxy/src/index.js",
    "apps/auth-broker/app/oauth/google/callback/page.tsx",
    "apps/auth-broker/lib/rust-api-proxy.ts",
    "deploy/helm/patchbay-auth-broker/values.yaml",
    "deploy/helm/patchbay-auth-broker/templates/deployment.yaml",
    "apps/web/features/auth/google-oauth.ts",
    "server-rs/crates/patchbay-handler/src/clerk_auth.rs",
    "apps/desktop/src/renderer/src/pages/login-handoff.ts",
    "apps/desktop/src/shared/runtime-config.ts",
  ]) {
    assert.equal(classifyAuthChange([path]).fullGoogleOAuthE2E, true);
  }
});

test("deduplicates and ignores empty path records", () => {
  assert.deepEqual(
    classifyAuthChange(["", "apps/auth-broker/app/globals.css", "apps/auth-broker/app/globals.css"]),
    { authBrokerRelease: true, fullGoogleOAuthE2E: false },
  );
});

test("broker-only image builds check out their source context", () => {
  assert.ok(
    ciWorkflow.includes(`      - name: Checkout
        if: \${{ needs.changes.outputs.frontend == 'true' || needs.changes.outputs.auth_broker_release == 'true' }}
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7`),
  );
});

test("CI always runs the accounts origin proxy contract tests", () => {
  assert.ok(
    ciWorkflow.includes(
      "node --test deploy/cloudflare/accounts-origin-proxy/src/index.test.mjs",
    ),
  );
});
