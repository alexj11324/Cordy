import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import YAML from "yaml";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("auth broker release remains an independently gated image", () => {
  const workflow = YAML.parse(read(".github/workflows/release.yml"));
  assert.ok(workflow.jobs["docker-auth-broker-build"]);
  assert.ok(workflow.jobs["docker-auth-broker-merge"]);
  assert.match(JSON.stringify(workflow.jobs["publish-release"].needs), /docker-auth-broker-merge/);
  assert.match(read("Dockerfile.auth-broker"), /pnpm --filter @patchbay\/auth-broker build/);
  const deployment = read("deploy/helm/patchbay-auth-broker/templates/deployment.yaml");
  for (const name of ["CLERK_PUBLISHABLE_KEY", "PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN", "PATCHBAY_ORIGIN_AUTH_TOKEN"]) assert.match(deployment, new RegExp(name));
  assert.match(deployment, /image\.digest must be an immutable sha256 digest/);
});

test("shipping contract names only the Go API authority", () => {
  const contract = JSON.parse(read("contracts/auth-broker/v1.json"));
  assert.equal(contract.origins.broker, "https://accounts.aspectlylabs.com");
  assert.equal(contract.origins.product, "https://patchbay.aspectlylabs.com");
  assert.equal(contract.origins.api, "https://api.aspectlylabs.com");
  assert.equal(contract.authority.patchbaySession, "go-api");
  assert.equal(contract.goApi.desktopRedeemPath, "/api/desktop-handoff/redeem");
  assert.doesNotMatch(JSON.stringify(contract), /rust/i);
});

test("desktop opens the accounts Google route directly", () => {
  const handoff = read("apps/desktop/src/renderer/src/pages/login-handoff.ts");
  assert.match(handoff, /new URL\(`\$\{accountsUrl\.replace\(\/\\\/\+\$\/, ""\)\}\/oauth\/google`\)/);
  assert.doesNotMatch(handoff, /localhost|\/login\?/);
});
