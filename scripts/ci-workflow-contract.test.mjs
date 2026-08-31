import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import test from "node:test";

const ci = await readFile(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
const release = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
const containerImages = await readFile(
  new URL("../.github/workflows/container-images.yml", import.meta.url),
  "utf8",
);
const desktopSmoke = await readFile(
  new URL("../.github/workflows/desktop-smoke.yml", import.meta.url),
  "utf8",
);
const dockerfile = await readFile(new URL("../Dockerfile", import.meta.url), "utf8");
const cacheCleanup = await readFile(
  new URL("./cleanup-actions-caches.mjs", import.meta.url),
  "utf8",
);
const workflowDirectory = new URL("../.github/workflows/", import.meta.url);

function stepBlocks(source) {
  const lines = source.split("\n");
  const blocks = [];
  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(/^(\s*)- name:/u);
    if (!match) continue;
    const indentation = match[1].length;
    let end = index + 1;
    while (end < lines.length) {
      const next = lines[end].match(/^(\s*)- name:/u);
      if (next && next[1].length === indentation) break;
      end += 1;
    }
    blocks.push(lines.slice(index, end).join("\n"));
  }
  return blocks;
}

test("Rust and Mobile validation are automatic path-classified merge gates", () => {
  assert.match(ci, /^\s{12}rust:\n/mu);
  assert.match(ci, /^\s{12}mobile:\n/mu);
  assert.match(ci, /RUST_CHANGED: \$\{\{ steps\.filter\.outputs\.rust \}\}/u);
  assert.match(ci, /MOBILE_CHANGED: \$\{\{ steps\.filter\.outputs\.mobile \}\}/u);
  assert.match(ci, /^  backend:\n/mu);
  assert.match(ci, /^  mobile:\n/mu);
  assert.match(ci, /^  frontend:\n/mu);
  assert.match(ci, /^  installer:\n/mu);
  assert.doesNotMatch(ci, /Rust validation was not manually requested/u);
  assert.match(ci, /- 'scripts\/verify-release-tag\.sh'/u);
});

test("Rust uses one workspace test invocation and PR compiler caches are read-only", () => {
  assert.match(ci, /cargo test --workspace --all-targets --locked/u);
  assert.doesNotMatch(ci, /cargo metadata --locked --no-deps/u);
  assert.match(ci, /key: cargo-downloads-.*hashFiles\('rust-toolchain\.toml'\)/u);
  assert.equal(
    [...ci.matchAll(/SCCACHE_GHA_RW_MODE: \$\{\{ github\.event_name == 'pull_request' && 'READ_ONLY' \|\| 'READ_WRITE' \}\}/gu)].length,
    3,
  );
});

test("Actions caches never store a Cargo target directory", async () => {
  const names = (await readdir(workflowDirectory)).filter((name) => name.endsWith(".yml"));
  for (const name of names) {
    const source = await readFile(new URL(name, workflowDirectory), "utf8");
    for (const block of stepBlocks(source).filter((value) => value.includes("uses: actions/cache@"))) {
      assert.doesNotMatch(block, /^\s+[-]?\s*server-rs\/target\/?\s*$/mu, `${name} caches server-rs/target`);
      assert.doesNotMatch(block, /^\s+[-]?\s*target\/?\s*$/mu, `${name} caches target`);
    }
  }
});

test("container builds do not export Rust target trees through Actions or BuildKit caches", () => {
  assert.doesNotMatch(containerImages, /buildkit-cache-dance/u);
  assert.doesNotMatch(containerImages, /\.buildkit-cache(?:\/|\\)/u);
  assert.doesNotMatch(dockerfile, /target=\/src\/server-rs\/target/u);
  assert.match(dockerfile, /rm -rf target/u);
});

test("formal release publication is gated to the current canonical repository", () => {
  assert.match(release, /github\.event\.repository\.id == 1341050282/u);
  assert.doesNotMatch(release, /github\.repository == ['"]patchbay-ai\/patchbay['"]/u);
});

test("Desktop smoke builds one exact CLI artifact per matrix target", () => {
  assert.match(desktopSmoke, /^  cli-build:\n/mu);
  assert.match(desktopSmoke, /desktop-smoke-cli-\$\{\{ matrix\.target \}\}-\$\{\{ matrix\.arch \}\}/u);
  assert.match(desktopSmoke, /PATCHBAY_PREBUILT_CLI_DIR: \$\{\{ runner\.temp \}\}\/patchbay-cli-bin/u);
  assert.match(desktopSmoke, /desktop-smoke-cargo-\$\{\{ runner\.os \}\}-\$\{\{ runner\.arch \}\}-\$\{\{ matrix\.rust_target \}\}-/u);
  assert.match(desktopSmoke, /mozilla-actions\/sccache-action@/u);
  assert.doesNotMatch(desktopSmoke, /path:[^\n]*target(?:\/|\\)/u);
});

test("every intermediate uploaded artifact expires after one day", async () => {
  const names = (await readdir(workflowDirectory)).filter((name) => name.endsWith(".yml"));
  for (const name of names) {
    const source = await readFile(new URL(name, workflowDirectory), "utf8");
    for (const block of stepBlocks(source).filter((value) => value.includes("uses: actions/upload-artifact@"))) {
      assert.match(block, /retention-days: 1/u, `${name} has an unbounded intermediate artifact`);
    }
  }
});

test("Turbo cache lifecycle is bounded for active, closed, and main refs", async () => {
  const closedPrWorkflow = await readFile(
    new URL("cache-maintenance.yml", workflowDirectory),
    "utf8",
  );
  assert.match(closedPrWorkflow, /workflow_run:/u);
  assert.match(closedPrWorkflow, /mode=prune-pr/u);
  assert.match(closedPrWorkflow, /mode=prune-main/u);
  assert.match(closedPrWorkflow, /--keep 2/u);
  assert.match(closedPrWorkflow, /types: \[closed\]/u);
  assert.match(closedPrWorkflow, /mode=delete-ref/u);
  assert.match(closedPrWorkflow, /pull-requests: read/u);
  assert.match(closedPrWorkflow, /gh api "repos\/\$GITHUB_REPOSITORY\/pulls\/\$RUN_PR_NUMBER"/u);
  assert.match(closedPrWorkflow, /if \[ "\$pr_state" = "closed" \]/u);
  assert.match(cacheCleanup, /AbortSignal\.timeout\(GITHUB_API_TIMEOUT_MS\)/u);
});

test("CI runs cache cleanup tests and classifies new development scripts", () => {
  assert.match(ci, /scripts\/cleanup-actions-caches\.test\.mjs/u);
  assert.equal([...ci.matchAll(/- 'scripts\/dev-\*\.mjs'/gu)].length, 2);
});

test("complete development auth contracts run outside path-filtered jobs", () => {
  const contractStep = stepBlocks(ci).find((block) =>
    block.includes("scripts/ci-workflow-contract.test.mjs"),
  );
  assert.ok(contractStep, "CI workflow contract step is missing");
  assert.match(contractStep, /scripts\/dev-clerk-auth\.test\.mjs/u);
  assert.match(contractStep, /scripts\/dev-auth-command\.test\.mjs/u);
  assert.match(contractStep, /scripts\/dev-runtime-command\.test\.mjs/u);
  assert.match(contractStep, /bash scripts\/dev-env\.test\.sh/u);
  assert.doesNotMatch(contractStep, /^\s+if:/mu);
  assert.doesNotMatch(
    ci,
    /vitest run[^\n]*scripts\/dev-(?:auth|runtime)-command\.test\.mjs/u,
    "node:test development contracts must not also be routed through Vitest",
  );
});

test("the obsolete fixed-commit Desktop artifact workflow is gone", async () => {
  const names = await readdir(workflowDirectory);
  assert.ok(!names.includes("aspectlylabs-desktop-artifact.yml"));
});
