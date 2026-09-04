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
  assert.match(
    read("Dockerfile.auth-broker"),
    /COPY --from=builder[^\n]*apps\/auth-broker\/public/u,
  );
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

test("shipping contract makes Guest and WebSocket isolation explicit", () => {
  const contract = JSON.parse(read("contracts/auth-broker/v1.json"));
  assert.deepEqual(contract.client, {
    clerkExchangePath: "/auth/clerk",
    guestPath: "/auth/guest",
    logoutPath: "/auth/logout",
    mePath: "/api/me",
    websocketPath: "/ws",
    guestTokenPrefix: "pbg_",
    guestWorkspaceAccess: false,
    guestWebsocketAccess: false,
  });
});

test("desktop opens the Accounts login surface directly", () => {
  const handoff = read("apps/desktop/src/renderer/src/pages/login-handoff.ts");
  assert.match(
    handoff,
    /new URL\(`\$\{accountsUrl\.replace\(\/\\\/\+\$\/, ""\)\}\/login`\)/,
  );
  assert.doesNotMatch(handoff, /localhost|\/oauth\/google/);
});

test("Accounts login uses the custom shadcn form instead of Clerk's card", () => {
  const page = read("apps/auth-broker/app/login/page.tsx");
  const form = read("apps/auth-broker/components/accounts-login-form.tsx");
  assert.match(read("apps/auth-broker/app/page.tsx"), /redirect\("\/login"\)/u);
  assert.doesNotMatch(page, /<SignIn\b/u);
  assert.match(form, /signIn\.emailCode\.sendCode/u);
  assert.match(form, /signIn\.emailCode\.verifyCode/u);
  assert.match(form, /Sign In with Email|emailButton/u);
});
