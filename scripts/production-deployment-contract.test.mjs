import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import test from "node:test";

const workflow = await readFile(
  new URL(
    "../.github/workflows/aspectlylabs-production-images.yml",
    import.meta.url,
  ),
  "utf8",
);
const agents = await readFile(new URL("../AGENTS.md", import.meta.url), "utf8");
const webConfig = await readFile(
  new URL("../apps/web/next.config.ts", import.meta.url),
  "utf8",
);
const docsConfig = await readFile(
  new URL("../apps/docs/next.config.mjs", import.meta.url),
  "utf8",
);
const authConfig = await readFile(
  new URL("../apps/auth-broker/next.config.ts", import.meta.url),
  "utf8",
);
const docsCompose = await readFile(
  new URL("../deploy/origin/production-docs.compose.yml", import.meta.url),
  "utf8",
);
const originNginx = await readFile(
  new URL("../deploy/origin/nginx/aspectlylabs-origin.conf", import.meta.url),
  "utf8",
);
const workflowDirectory = new URL("../.github/workflows/", import.meta.url);

test("production follows successful main CI instead of a temporary deployment branch", () => {
  assert.match(workflow, /workflow_run:\n\s+workflows: \[CI\]/u);
  assert.match(workflow, /branches: \[main\]/u);
  assert.match(
    workflow,
    /github\.event\.workflow_run\.conclusion == 'success'/u,
  );
  assert.match(workflow, /github\.event\.workflow_run\.event == 'push'/u);
  assert.match(
    workflow,
    /github\.event\.workflow_run\.head_repository\.full_name == github\.repository/u,
  );
  assert.doesNotMatch(workflow, /workflow_dispatch/u);
  assert.doesNotMatch(workflow, /codex\/aspectlylabs-fce7-build/u);
});

test("every production run builds the complete image set from one SHA", () => {
  for (const name of ["backend", "web", "docs", "auth-broker"]) {
    assert.match(workflow, new RegExp(`- name: ${name}\\n`, "u"));
    assert.match(
      workflow,
      new RegExp(`production-image-\\$\\{\\{ matrix\\.name \\}\\}`, "u"),
    );
  }
  assert.doesNotMatch(workflow, /inputs\.image/u);
  assert.doesNotMatch(workflow, /paths:/u);
  assert.match(workflow, /assemble-production-manifest\.mjs/u);
  assert.match(workflow, /Verify every immutable image is pullable/u);
});

test("production uses a protected serialized deployment with rollback and runtime gates", () => {
  assert.match(workflow, /group: aspectlylabs-production/u);
  assert.match(workflow, /cancel-in-progress: false/u);
  assert.match(
    workflow,
    /name: production\n\s+url: https:\/\/patchbay\.aspectlylabs\.com/u,
  );
  assert.match(workflow, /PRODUCTION_SSH_PRIVATE_KEY/u);
  assert.match(workflow, /StrictHostKeyChecking=yes/u);
  assert.match(workflow, /action: "rollback"/u);
  assert.match(workflow, /schema_version: 2/u);
  assert.match(workflow, /failed_workflow_run_id/u);
  assert.match(workflow, /\.unchanged == true/u);
  assert.match(workflow, /verify-production-deployment\.mjs/u);
  assert.match(workflow, /verify-production-browser\.mjs/u);
  assert.match(workflow, /playwright install --with-deps chromium/u);
  assert.match(workflow, /browser_auth\.sign_in_ticket/u);
  assert.doesNotMatch(workflow, /CLERK_SECRET_KEY/u);
});

test("obsolete partial image publication workflows are removed", async () => {
  const names = await readdir(workflowDirectory);
  assert.ok(!names.includes("container-images.yml"));
  assert.ok(!names.includes("auth-broker-release.yml"));
});

test("all public Next services expose the immutable build fingerprint", () => {
  for (const source of [webConfig, docsConfig, authConfig]) {
    assert.match(source, /X-Patchbay-Build/u);
    assert.match(source, /NEXT_PUBLIC_APP_VERSION/u);
  }
});

test("the production Docs healthcheck uses the runtime's Node executable", () => {
  assert.match(docsCompose, /test:\n\s+- CMD\n\s+- node\n/u);
  assert.doesNotMatch(docsCompose, /wget/u);
});

test("the production origin routes Docs before the Web catch-all", () => {
  const exactDocs = originNginx.indexOf("location = /docs {");
  const nestedDocs = originNginx.indexOf("location ^~ /docs/ {");
  const webCatchAll = originNginx.indexOf("location / {");

  assert.ok(exactDocs >= 0);
  assert.ok(nestedDocs > exactDocs);
  assert.ok(webCatchAll > nestedDocs);
  for (const blockStart of [exactDocs, nestedDocs]) {
    const blockEnd = originNginx.indexOf("\n    }", blockStart);
    const block = originNginx.slice(blockStart, blockEnd);
    assert.match(block, /proxy_pass http:\/\/127\.0\.0\.1:4000;/u);
    assert.match(block, /proxy_set_header X-Forwarded-Proto https;/u);
  }
});

test("agents isolate main and perform safe post-merge notification", () => {
  assert.match(
    agents,
    /primary `main` checkout is a synchronization-only baseline/u,
  );
  assert.match(agents, /dedicated branch plus worktree/u);
  assert.match(
    agents,
    /fast-forward the\n\s+primary `main` checkout only when that checkout is clean/u,
  );
  assert.match(agents, /notify every running task\/thread/u);
  assert.match(
    agents,
    /Never rebase, reset, or otherwise\n\s+rewrite an active task worktree/u,
  );
});
