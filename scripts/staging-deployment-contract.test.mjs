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
]);

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

test("staging compose overlays never reattach production projects", () => {
  assert.match(stagingDocs, /name: patchbay-staging-docs/u);
  assert.match(stagingBroker, /name: patchbay-staging-auth-broker/u);
  assert.match(stagingDocs, /127\.0\.0\.1:4001:3000/u);
  assert.match(stagingBroker, /127\.0\.0\.1:43101:3000/u);
  assert.doesNotMatch(`${stagingDocs}\n${stagingBroker}\n${stagingOverride}`, /cordy632|cordy\b/u);
  assert.match(
    stagingOverride,
    /CLERK_AUTHORIZED_PARTIES: https:\/\/accounts\.staging\.aspectlylabs\.com,https:\/\/staging\.aspectlylabs\.com/u,
  );
  assert.match(stagingOverride, /PATCHBAY_APP_URL to https:\/\/staging\.aspectlylabs\.com/u);
  assert.doesNotMatch(stagingOverride, /patchbay\.aspectlylabs\.com/u);
});

test("staging gateway refuses production state", () => {
  assert.match(stagingGateway, /DEFAULT_ROOT = Path\("\/var\/lib\/patchbay-staging"\)/u);
  assert.match(stagingGateway, /PRODUCTION_ROOT = Path\("\/var\/lib\/patchbay-production"\)/u);
  assert.match(stagingGateway, /FORBIDDEN_COMPOSE_PROJECTS = \{"cordy632", "cordy", "patchbay-auth-broker"\}/u);
  assert.match(stagingGateway, /PRODUCT_COMPOSE_PROJECT = "patchbay-staging"/u);
  assert.match(stagingGateway, /STAGING_SMOKE_USER_EMAIL = "staging-smoke@aspectlylabs.com"/u);
  assert.match(installer, /patchbay-staging-github-actions/u);
  assert.match(installer, /\/usr\/local\/bin\/patchbay-staging-deploy/u);
  assert.doesNotMatch(installer, /cordy632-backend-1/u);
  assert.doesNotMatch(stagingGateway, /cordy632-backend-1/u);
  assert.match(productionGateway, /DEFAULT_ROOT = Path\("\/var\/lib\/patchbay-production"\)/u);
  assert.doesNotMatch(productionGateway, /patchbay-staging/u);
});
