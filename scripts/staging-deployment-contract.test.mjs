import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function read(path) {
  return readFile(new URL(`../${path}`, import.meta.url), "utf8");
}

const hosted = JSON.parse(
  await readFile(new URL("../deploy/origin/hosted-environments.json", import.meta.url), "utf8"),
);

const [
  workflow,
  productionWorkflow,
  originNginx,
  stagingOverride,
  stagingDocs,
  stagingBroker,
  stagingGateway,
  installer,
  productionGateway,
  accountsWorker,
  environmentsDoc,
] = await Promise.all([
  read(".github/workflows/aspectlylabs-staging.yml"),
  read(".github/workflows/aspectlylabs-production-images.yml"),
  read("deploy/origin/nginx/aspectlylabs-origin.conf"),
  read("deploy/origin/staging-product.override.yml"),
  read("deploy/origin/staging-docs.compose.yml"),
  read("deploy/origin/staging-auth-broker.compose.yml"),
  read("deploy/origin/staging_deploy.py"),
  read("deploy/origin/install-staging-deploy.sh"),
  read("deploy/origin/production_deploy.py"),
  read("deploy/cloudflare/accounts-origin-proxy/wrangler.toml"),
  read("docs/operations/environments.md"),
]);

test("synthetic login initiates the channel-specific handoff before broker registration", async () => {
  const source = await read("scripts/verify-production-browser.mjs");
  const flow = source.slice(source.indexOf("async function redeemSyntheticLogin("));
  const initiate = flow.indexOf("/api/desktop-handoff/initiate");
  assert.ok(initiate >= 0);
  assert.ok(initiate < flow.indexOf("/v1/desktop/google/attempt"));
  assert.match(flow.slice(initiate, flow.indexOf("const registered")), /callback_protocol: DESKTOP_CALLBACK_PROTOCOL/u);
});

test("staging is a separate GitHub Environment from production", () => {
  assert.match(workflow, /environment:\n\s+name: staging/u);
  assert.match(workflow, /STAGING_SSH_PRIVATE_KEY/u);
  assert.match(productionWorkflow, /environment:\n\s+name: production/u);
  assert.doesNotMatch(workflow, /PRODUCTION_SSH_PRIVATE_KEY/u);
  assert.doesNotMatch(productionWorkflow, /environment:\n\s+name: staging/u);
  assert.doesNotMatch(productionWorkflow, /aspectlylabs-staging/u);
  assert.match(workflow, /group: aspectlylabs-staging/u);
  assert.match(productionWorkflow, /group: aspectlylabs-production/u);
  assert.notEqual(
    workflow.match(/group: aspectlylabs-staging/u)?.[0],
    productionWorkflow.match(/group: aspectlylabs-production/u)?.[0],
  );
});

test("staging follows a successful production run and cannot gate it", () => {
  assert.match(workflow, /workflow_run:\n\s+workflows: \[Aspectlylabs production\]/u);
  assert.match(workflow, /branches: \[main\]/u);
  assert.match(workflow, /workflow_run\.conclusion == 'success'/u);
  assert.match(
    workflow,
    /workflow_run\.head_repository\.full_name == github\.repository/u,
  );
  assert.match(workflow, /workflow_dispatch:/u);
  assert.doesNotMatch(productionWorkflow, /needs:.*staging/u);
});

test("staging consumes published production images instead of rebuilding", () => {
  assert.match(workflow, /workflows: \[Aspectlylabs production\]/u);
  assert.doesNotMatch(workflow, /docker\/build-push-action/u);
  assert.match(workflow, /assemble-production-manifest\.mjs/u);
  assert.match(workflow, /resolve-published-image-records\.mjs/u);
  assert.match(workflow, /verify-staging-deployment\.mjs/u);
  assert.match(workflow, /verify-production-browser\.mjs/u);
  assert.match(workflow, /ORVILO_VERIFY_ENV: staging/u);
  assert.match(workflow, /browser_auth\.sign_in_ticket/u);
  assert.match(workflow, /browser_auth\.testing_token/u);
});

test("staging origin routing uses isolated loopback ports", () => {
  const { staging, production } = hosted.environments;
  assert.match(
    originNginx,
    /server_name api\.staging\.aspectlylabs\.com;[\s\S]*proxy_pass http:\/\/127\.0\.0\.1:8211/u,
  );
  assert.match(
    originNginx,
    /server_name staging\.aspectlylabs\.com;[\s\S]*proxy_pass http:\/\/127\.0\.0\.1:3111/u,
  );
  assert.match(
    originNginx,
    /server_name staging\.aspectlylabs\.com;[\s\S]*proxy_pass http:\/\/127\.0\.0\.1:4001/u,
  );
  assert.match(
    originNginx,
    /server_name accounts-origin\.staging\.aspectlylabs\.com;[\s\S]*proxy_pass http:\/\/127\.0\.0\.1:43101/u,
  );
  assert.match(
    originNginx,
    /server_name api\.aspectlylabs\.com;[\s\S]*proxy_pass http:\/\/127\.0\.0\.1:8210/u,
  );
  assert.equal(staging.ports.backend, 8211);
  assert.equal(production.ports.backend, 8210);
  const stagingApiMarker = "server_name api.staging.aspectlylabs.com;";
  const stagingApiStart = originNginx.indexOf(stagingApiMarker);
  assert.notEqual(stagingApiStart, -1);
  const stagingApiEnd = originNginx.indexOf("\n}", stagingApiStart);
  const stagingApiBlock = originNginx.slice(
    stagingApiStart,
    stagingApiEnd === -1 ? undefined : stagingApiEnd + 2,
  );
  assert.match(stagingApiBlock, /server_name api\.staging\.aspectlylabs\.com;/u);
  assert.doesNotMatch(stagingApiBlock, /127\.0\.0\.1:8210/u);
});

test("staging Accounts uses its own Cloudflare Worker route and origin", () => {
  assert.match(accountsWorker, /\[env\.staging\]/u);
  assert.match(accountsWorker, /name = "aspectlylabs-proxy-staging"/u);
  assert.match(accountsWorker, /pattern = "accounts\.staging\.aspectlylabs\.com"/u);
  assert.match(accountsWorker, /ORIGIN = "https:\/\/accounts-origin\.staging\.aspectlylabs\.com"/u);
  assert.doesNotMatch(accountsWorker, /\[env\.staging\.vars\][\s\S]*accounts-origin\.aspectlylabs\.com/u);
  const stagingLimitIds = [
    ...accountsWorker.matchAll(
      /\[\[env\.staging\.ratelimits\]\]\s*\nname = "[^"]+"\s*\nnamespace_id = "(\d+)"/g,
    ),
  ].map((match) => match[1]);
  const productionLimitIds = [
    ...accountsWorker.matchAll(
      /(?:^|\n)\[\[ratelimits\]\]\s*\nname = "[^"]+"\s*\nnamespace_id = "(\d+)"/g,
    ),
  ].map((match) => match[1]);
  assert.deepEqual(stagingLimitIds, ["21410502821", "21410502822"]);
  assert.deepEqual(productionLimitIds, ["13410502821", "13410502822"]);
  assert.equal(
    new Set([...stagingLimitIds, ...productionLimitIds]).size,
    stagingLimitIds.length + productionLimitIds.length,
  );
  assert.match(environmentsDoc, /wrangler secret put ORIGIN_AUTH_TOKEN --env staging/u);
  assert.match(environmentsDoc, /wrangler deploy --env staging/u);
  assert.match(environmentsDoc, /NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY/u);
});

test("staging compose overlays never reattach production projects", () => {
  assert.match(stagingDocs, /name: orvilo-staging-docs/u);
  assert.match(stagingBroker, /name: orvilo-staging-auth-broker/u);
  assert.match(stagingDocs, /127\.0\.0\.1:4001:3000/u);
  assert.match(stagingBroker, /127\.0\.0\.1:43101:3000/u);
  assert.doesNotMatch(`${stagingDocs}\n${stagingBroker}\n${stagingOverride}`, /cordy632|cordy\b/u);
  assert.match(
    stagingOverride,
    /CLERK_AUTHORIZED_PARTIES: https:\/\/accounts\.staging\.aspectlylabs\.com,https:\/\/staging\.aspectlylabs\.com/u,
  );
  assert.match(stagingOverride, /ORVILO_APP_URL to https:\/\/staging\.aspectlylabs\.com/u);
  assert.match(stagingOverride, /ANALYTICS_ENVIRONMENT: staging/u);
  assert.match(stagingOverride, /ALLOW_SIGNUP: "false"/u);
  assert.match(stagingOverride, /ALLOWED_EMAILS: \$\{ALLOWED_EMAILS:\?/u);
  assert.match(stagingOverride, /ALLOWED_EMAIL_DOMAINS: ""/u);
  for (const key of ["ORVILO_API_ORIGIN", "ORVILO_AUTH_BROKER_ORIGIN", "ORVILO_DESKTOP_BROKER_AUTH_TOKEN", "ORVILO_ORIGIN_AUTH_TOKEN"]) {
    assert.match(stagingBroker, new RegExp(`\\n      ${key}:`));
  }
  assert.match(stagingOverride, /\n      ORVILO_DESKTOP_BROKER_AUTH_TOKEN:/u);
  assert.match(stagingOverride, /\n      ORVILO_CLERK_PUBLISHABLE_KEY:/u);
  assert.doesNotMatch(`${stagingOverride}\n${stagingBroker}`, /PATCHBAY_/u);
  assert.doesNotMatch(stagingOverride, /orvilo\.aspectlylabs\.com/u);
});

function originServerBlock(source, serverName) {
  const marker = `server_name ${serverName};`;
  const start = source.indexOf(marker);
  assert.notEqual(start, -1, marker);
  const end = source.indexOf("\n}", start);
  return source.slice(start, end === -1 ? undefined : end + 2);
}

test("staging accounts origin validates its own Worker token", () => {
  const staging = originServerBlock(
    originNginx,
    "accounts-origin.staging.aspectlylabs.com",
  );
  const production = originServerBlock(
    originNginx,
    "accounts-origin.aspectlylabs.com",
  );
  assert.match(
    staging,
    /include \/etc\/nginx\/snippets\/orvilo-staging-accounts-origin-auth\.conf;/u,
  );
  assert.doesNotMatch(staging, /snippets\/orvilo-accounts-origin-auth\.conf/u);
  assert.match(
    production,
    /include \/etc\/nginx\/snippets\/orvilo-accounts-origin-auth\.conf;/u,
  );
  assert.doesNotMatch(production, /orvilo-staging-accounts-origin-auth/u);
  assert.match(environmentsDoc, /orvilo-staging-accounts-origin-auth\.conf/u);
  assert.match(environmentsDoc, /orvilo-accounts-origin-auth\.conf/u);
  assert.match(environmentsDoc, /\$http_x_orvilo_origin_auth/u);
  assert.match(environmentsDoc, /wrangler secret put ORIGIN_AUTH_TOKEN --env staging/u);
});

test("staging gateway refuses production state", () => {
  assert.ok(installer.indexOf('chown "$deploy_user:$deploy_group" "$secret_file"') < installer.indexOf("--bootstrap"));
  assert.match(installer, /chmod 0600 "\$secret_file"/u);
  assert.match(installer, /\[ -L "\$secret_file" \]/u);
  for (const file of ["staging-product.override.yml", "staging-docs.compose.yml", "staging-auth-broker.compose.yml"]) {
    assert.ok(stagingGateway.includes(`str(release / "deploy/origin/${file}")`));
    assert.ok(!stagingGateway.includes(`str(self.static_directory / "${file}")`));
  }
  assert.match(stagingGateway, /DEFAULT_ROOT = Path\("\/var\/lib\/orvilo-staging"\)/u);
  assert.match(stagingGateway, /PRODUCTION_ROOT = Path\("\/var\/lib\/orvilo-production"\)/u);
  assert.match(stagingGateway, /FORBIDDEN_COMPOSE_PROJECTS = \{"cordy632", "cordy", "orvilo-auth-broker"\}/u);
  assert.match(stagingGateway, /PRODUCT_COMPOSE_PROJECT = "orvilo-staging"/u);
  assert.match(stagingGateway, /STAGING_SMOKE_USER_EMAIL = "staging-smoke@aspectlylabs.com"/u);
  assert.match(installer, /orvilo-staging-github-actions/u);
  assert.match(installer, /\/usr\/local\/bin\/orvilo-staging-deploy/u);
  assert.doesNotMatch(installer, /cordy632-backend-1/u);
  assert.doesNotMatch(stagingGateway, /cordy632-backend-1/u);
  assert.match(productionGateway, /DEFAULT_ROOT = Path\("\/var\/lib\/orvilo-production"\)/u);
  assert.doesNotMatch(productionGateway, /orvilo-staging/u);
});
