import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function read(path) {
  return readFile(new URL(`../${path}`, import.meta.url), "utf8");
}

const [
  workflow,
  release,
  backendDockerfile,
  webDockerfile,
  docsDockerfile,
  brokerDockerfile,
  webConfig,
  docsConfig,
  brokerConfig,
  webLayout,
  deployGateway,
  originNginx,
  productionOverride,
] = await Promise.all([
  read(".github/workflows/aspectlylabs-production-images.yml"),
  read(".github/workflows/release.yml"),
  read("Dockerfile"),
  read("Dockerfile.web"),
  read("Dockerfile.docs"),
  read("Dockerfile.auth-broker"),
  read("apps/web/next.config.ts"),
  read("apps/docs/next.config.mjs"),
  read("apps/auth-broker/next.config.ts"),
  read("apps/web/app/layout.tsx"),
  read("deploy/origin/production_deploy.py"),
  read("deploy/origin/nginx/aspectlylabs-origin.conf"),
  read("deploy/origin/production-product.override.yml"),
]);

// The manifest assembler takes four positional operands and validates none of
// them as a path. A mangled shell line continuation still parses as valid bash
// and still exits 0 on the workflow's own `bash -n`, so assert the argument
// vector itself rather than the mere presence of the command.
function manifestAssemblyOperands(source) {
  const invocation = source.match(
    /node scripts\/assemble-production-manifest\.mjs((?:[^\n]*\\\n)*[^\n]*)\n/u,
  );
  assert.ok(invocation, "production must assemble a deployment manifest");
  return invocation[1]
    .replaceAll(/\$\{\{\s*([^}]*?)\s*\}\}/gu, "<$1>")
    .replaceAll("\\\n", " ")
    .split(/\s+/u)
    .filter(Boolean);
}

test("production follows the successful current main CI run", () => {
  assert.match(workflow, /workflow_run:\n\s+workflows: \[CI\]/u);
  assert.match(workflow, /branches: \[main\]/u);
  assert.match(workflow, /workflow_run\.conclusion == 'success'/u);
  assert.match(workflow, /workflow_run\.event == 'push'/u);
  assert.match(
    workflow,
    /workflow_run\.head_repository\.full_name == github\.repository/u,
  );
  assert.match(workflow, /source_sha[\s\S]*main_sha/u);
  assert.doesNotMatch(workflow, /workflow_dispatch/u);
});

test("tag release remains separate from production deployment", () => {
  assert.match(release, /push:\n\s+tags:/u);
  assert.doesNotMatch(release, /workflow_run:/u);
  assert.doesNotMatch(release, /deploy-production/u);
  assert.doesNotMatch(workflow, /refs\/tags/u);
});

test("one source SHA produces the complete immutable image set", () => {
  for (const name of ["backend", "web", "docs", "auth-broker"]) {
    assert.match(workflow, new RegExp(`- name: ${name}\\n`, "u"));
  }
  assert.match(workflow, /assemble-production-manifest\.mjs/u);
  assert.deepEqual(manifestAssemblyOperands(workflow), [
    "/tmp/production-images",
    "<needs.resolve-source.outputs.sha>",
    "/tmp/production-manifest.json",
    "<github.repository_owner>",
  ]);
  assert.match(workflow, /Verify every immutable image is pullable/u);
  assert.match(workflow, /sha256sum production-manifest\.json/u);
  assert.match(workflow, /sha256sum --check production-manifest\.json\.sha256/u);
  assert.doesNotMatch(workflow, /paths:/u);
});

test("production backend is the Go server with no Rust runtime contract", () => {
  assert.match(backendDockerfile, /FROM golang:1\.26-alpine AS builder/u);
  assert.match(backendDockerfile, /go build[\s\S]*\.\/cmd\/server/u);
  for (const source of [workflow, deployGateway, backendDockerfile]) {
    assert.doesNotMatch(source, /server-rs|cargo|Dockerfile\.rust-runtime/u);
  }
});

test("all four services expose matching build and commit fingerprints", () => {
  assert.match(workflow, /VERSION=sha-\$\{\{ needs\.resolve-source\.outputs\.sha \}\}/u);
  assert.match(workflow, /COMMIT=\$\{\{ needs\.resolve-source\.outputs\.sha \}\}/u);
  for (const dockerfile of [webDockerfile, docsDockerfile, brokerDockerfile]) {
    assert.match(dockerfile, /ARG COMMIT_SHA=unknown/u);
    assert.match(dockerfile, /NEXT_PUBLIC_COMMIT_SHA/u);
  }
  for (const config of [webConfig, docsConfig, brokerConfig]) {
    assert.match(config, /X-Patchbay-Build/u);
    assert.match(config, /X-Patchbay-Commit/u);
  }
  assert.match(deployGateway, /expected_commit/u);
});

test("deployment is serialized, protected, and rollback-bound", () => {
  assert.match(workflow, /group: aspectlylabs-production/u);
  assert.match(workflow, /cancel-in-progress: false/u);
  assert.match(workflow, /environment:\n\s+name: production/u);
  assert.match(workflow, /PRODUCTION_SSH_PRIVATE_KEY/u);
  assert.match(workflow, /StrictHostKeyChecking=yes/u);
  assert.match(workflow, /schema_version: 2/u);
  assert.match(workflow, /failed_workflow_run_id/u);
  assert.match(deployGateway, /current_main != source_sha/u);
  assert.match(deployGateway, /automatic rollback/u);
});

test("public routing uses the Aspectly Labs product domains", () => {
  for (const domain of [
    "api.aspectlylabs.com",
    "patchbay.aspectlylabs.com",
    "accounts.aspectlylabs.com",
  ]) {
    assert.match(`${workflow}\n${originNginx}`, new RegExp(domain.replaceAll(".", "\\."), "u"));
  }
  assert.doesNotMatch(`${workflow}\n${originNginx}`, /patchbay\.ai/u);
  assert.match(
    originNginx,
    /server_name patchbay\.aspectlylabs\.com;[\s\S]*location \^~ \/docs\//u,
  );
  assert.match(
    originNginx,
    /server_name api\.aspectlylabs\.com;[\s\S]*proxy_pass http:\/\/127\.0\.0\.1:8210/u,
  );
  assert.match(
    originNginx,
    /proxy_set_header X-Patchbay-Origin-Auth "";/u,
  );
  assert.match(
    originNginx,
    /proxy_set_header X-Patchbay-Desktop-Broker-Auth "";/u,
  );
});

test("public and authenticated browser acceptance gate success", () => {
  assert.match(workflow, /verify-production-deployment\.mjs/u);
  assert.match(workflow, /verify-production-browser\.mjs/u);
  assert.match(workflow, /playwright install --with-deps chromium/u);
  assert.match(workflow, /browser_auth\.sign_in_ticket/u);
  assert.match(workflow, /browser_auth\.testing_token/u);
});

test("the deployed Web Clerk provider is runtime-configured and accepts both browser origins", () => {
  assert.match(webLayout, /PATCHBAY_CLERK_PUBLISHABLE_KEY/u);
  assert.match(
    deployGateway,
    /CLERK_PUBLISHABLE_KEY[\s\S]*PATCHBAY_CLERK_PUBLISHABLE_KEY/u,
  );
  assert.match(
    productionOverride,
    /CLERK_AUTHORIZED_PARTIES: https:\/\/accounts\.aspectlylabs\.com,https:\/\/patchbay\.aspectlylabs\.com/u,
  );
  assert.match(productionOverride, /PATCHBAY_CLERK_PUBLISHABLE_KEY/u);
});
