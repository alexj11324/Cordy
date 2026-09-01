import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import test from "node:test";

const ci = await readFile(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
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

test("merge queue runs the same path-aware CI gate", () => {
  assert.match(ci, /^  merge_group:\n/mu);
  assert.match(ci, /^    types: \[checks_requested\]\n/mu);
  assert.match(ci, /\$EVENT_NAME" = "merge_group"/u);
  assert.equal(
    [...ci.matchAll(/github\.event_name == 'pull_request' \|\| github\.event_name == 'merge_group'/gu)].length,
    3,
  );
  assert.match(ci, /cancel-in-progress: true/u);
});

test("workflow-only changes keep the four required aggregates green without heavy jobs", () => {
  assert.doesNotMatch(ci, /- '\.github\/workflows\/ci\.yml'/u);
  for (const message of [
    "Frontend validation intentionally skipped: no relevant paths changed.",
    "Rust validation intentionally skipped: no relevant paths changed.",
    "Mobile validation intentionally skipped: no relevant paths changed.",
    "Installer validation intentionally skipped: no relevant paths changed.",
  ]) {
    assert.match(ci, new RegExp(message.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&"), "u"));
  }
});

test("Rust uses one workspace test invocation and PR compiler caches are read-only", () => {
  assert.match(ci, /cargo test --workspace --all-targets --locked/u);
  assert.doesNotMatch(ci, /cargo metadata --locked --no-deps/u);
  assert.equal(
    [...ci.matchAll(/SCCACHE_GHA_RW_MODE: \$\{\{ \(github\.event_name == 'pull_request' \|\| github\.event_name == 'merge_group'\) && 'READ_ONLY' \|\| 'READ_WRITE' \}\}/gu)].length,
    3,
  );
});

test("Rust quality uses clippy as the compile gate and CI compiles with line tables only", () => {
  assert.match(ci, /cargo clippy --workspace --all-targets --locked -- -D warnings/u);
  assert.match(
    ci,
    /cargo build --locked -p patchbay-server --bin patchbay-server -p patchbay-cli --bin patchbay/u,
  );
  assert.doesNotMatch(ci, /cargo check --workspace --all-targets --locked/u);
  assert.doesNotMatch(ci, /cargo build --workspace --locked/u);
  assert.equal(
    [...ci.matchAll(/CARGO_INCREMENTAL: "0"/gu)].length,
    3,
  );
  assert.equal(
    [...ci.matchAll(/CARGO_PROFILE_DEV_DEBUG: "line-tables-only"/gu)].length,
    3,
  );
  assert.equal(
    [...ci.matchAll(/CARGO_PROFILE_TEST_DEBUG: "line-tables-only"/gu)].length,
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

test("secure development auth bootstrap runs outside path-filtered jobs", () => {
  const contractStep = stepBlocks(ci).find((block) =>
    block.includes("scripts/ci-workflow-contract.test.mjs"),
  );
  assert.ok(contractStep, "CI workflow contract step is missing");
  assert.match(contractStep, /scripts\/dev-clerk-auth\.test\.mjs/u);
  assert.doesNotMatch(contractStep, /^\s+if:/mu);
});

test("the obsolete fixed-commit Desktop artifact workflow is gone", async () => {
  const names = await readdir(workflowDirectory);
  assert.ok(!names.includes("aspectlylabs-desktop-artifact.yml"));
});

test("the Stack CI runbook keeps queue setup outside source-controlled gates", async () => {
  const runbook = await readFile(new URL("../.github/STACK_CI.md", import.meta.url), "utf8");
  assert.match(runbook, /gh stack merge .*--yes/u);
  assert.match(runbook, /merge_group/u);
  assert.match(runbook, /frontend.*backend.*mobile.*installer/su);
  assert.match(runbook, /required checks/u);
  assert.match(runbook, /cannot be enabled by a workflow commit/u);
});
